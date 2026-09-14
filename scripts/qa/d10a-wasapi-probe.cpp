#define NOMINMAX
#include <windows.h>
#include <initguid.h>
#include <audioclient.h>
#include <bcrypt.h>
#include <endpointvolume.h>
#include <propkeydef.h>
#include <functiondiscoverykeys_devpkey.h>
#include <ks.h>
#include <ksmedia.h>
#include <mmdeviceapi.h>
#include <mmreg.h>
#include <propvarutil.h>

#include <algorithm>
#include <atomic>
#include <cmath>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <iomanip>
#include <iostream>
#include <map>
#include <memory>
#include <mutex>
#include <optional>
#include <sstream>
#include <string>
#include <thread>
#include <vector>

#pragma comment(lib, "bcrypt.lib")
#pragma comment(lib, "ole32.lib")
#pragma comment(lib, "propsys.lib")
#pragma comment(lib, "uuid.lib")

namespace fs = std::filesystem;

template <typename T>
class ComPtr {
public:
    ComPtr() = default;
    ~ComPtr() { reset(); }
    ComPtr(const ComPtr&) = delete;
    ComPtr& operator=(const ComPtr&) = delete;
    ComPtr(ComPtr&& other) noexcept : ptr_(other.ptr_) { other.ptr_ = nullptr; }
    ComPtr& operator=(ComPtr&& other) noexcept {
        if (this != &other) {
            reset();
            ptr_ = other.ptr_;
            other.ptr_ = nullptr;
        }
        return *this;
    }
    T* get() const { return ptr_; }
    T** put() {
        reset();
        return &ptr_;
    }
    T* operator->() const { return ptr_; }
    explicit operator bool() const { return ptr_ != nullptr; }
    void reset(T* value = nullptr) {
        if (ptr_) ptr_->Release();
        ptr_ = value;
    }
private:
    T* ptr_ = nullptr;
};

struct CoInit {
    HRESULT hr;
    explicit CoInit(DWORD flags = COINIT_MULTITHREADED) : hr(CoInitializeEx(nullptr, flags)) {}
    ~CoInit() { if (SUCCEEDED(hr)) CoUninitialize(); }
};

static std::string utf8(const std::wstring& value) {
    if (value.empty()) return {};
    int count = WideCharToMultiByte(CP_UTF8, 0, value.data(), static_cast<int>(value.size()), nullptr, 0, nullptr, nullptr);
    std::string result(static_cast<size_t>(count), '\0');
    WideCharToMultiByte(CP_UTF8, 0, value.data(), static_cast<int>(value.size()), result.data(), count, nullptr, nullptr);
    return result;
}

static std::wstring widen(const std::string& value) {
    if (value.empty()) return {};
    int count = MultiByteToWideChar(CP_UTF8, 0, value.data(), static_cast<int>(value.size()), nullptr, 0);
    std::wstring result(static_cast<size_t>(count), L'\0');
    MultiByteToWideChar(CP_UTF8, 0, value.data(), static_cast<int>(value.size()), result.data(), count);
    return result;
}

static std::string jsonEscape(const std::string& value) {
    std::ostringstream out;
    for (unsigned char ch : value) {
        switch (ch) {
        case '\"': out << "\\\""; break;
        case '\\': out << "\\\\"; break;
        case '\b': out << "\\b"; break;
        case '\f': out << "\\f"; break;
        case '\n': out << "\\n"; break;
        case '\r': out << "\\r"; break;
        case '\t': out << "\\t"; break;
        default:
            if (ch < 0x20) {
                out << "\\u" << std::hex << std::setw(4) << std::setfill('0') << static_cast<int>(ch) << std::dec;
            } else {
                out << static_cast<char>(ch);
            }
        }
    }
    return out.str();
}

static std::string quote(const std::string& value) { return "\"" + jsonEscape(value) + "\""; }

static std::string isoUtcNow() {
    SYSTEMTIME st{};
    GetSystemTime(&st);
    std::ostringstream out;
    out << std::setfill('0') << std::setw(4) << st.wYear << '-' << std::setw(2) << st.wMonth << '-'
        << std::setw(2) << st.wDay << 'T' << std::setw(2) << st.wHour << ':' << std::setw(2) << st.wMinute
        << ':' << std::setw(2) << st.wSecond << '.' << std::setw(3) << st.wMilliseconds << 'Z';
    return out.str();
}

static uint64_t qpcFrequency() {
    LARGE_INTEGER value{};
    QueryPerformanceFrequency(&value);
    return static_cast<uint64_t>(value.QuadPart);
}

static uint64_t qpcNowRaw() {
    LARGE_INTEGER value{};
    QueryPerformanceCounter(&value);
    return static_cast<uint64_t>(value.QuadPart);
}

static uint64_t qpcRawToNs(uint64_t raw) {
    const long double scaled = static_cast<long double>(raw) * 1000000000.0L / static_cast<long double>(qpcFrequency());
    return static_cast<uint64_t>(scaled);
}

static uint64_t qpcNowNs() { return qpcRawToNs(qpcNowRaw()); }

static std::string hrHex(HRESULT hr) {
    std::ostringstream out;
    out << "0x" << std::uppercase << std::hex << std::setw(8) << std::setfill('0') << static_cast<uint32_t>(hr);
    return out.str();
}

static std::string stateName(DWORD state) {
    if (state == DEVICE_STATE_ACTIVE) return "active";
    if (state == DEVICE_STATE_DISABLED) return "disabled";
    if (state == DEVICE_STATE_NOTPRESENT) return "not_present";
    if (state == DEVICE_STATE_UNPLUGGED) return "unplugged";
    return "unknown_" + std::to_string(state);
}

static std::string roleName(ERole role) {
    switch (role) {
    case eConsole: return "console";
    case eMultimedia: return "multimedia";
    case eCommunications: return "communications";
    default: return "unknown";
    }
}

static std::string guidString(const GUID& guid) {
    wchar_t buffer[64]{};
    StringFromGUID2(guid, buffer, static_cast<int>(std::size(buffer)));
    return utf8(buffer);
}

static std::string endpointId(IMMDevice* device) {
    LPWSTR value = nullptr;
    HRESULT hr = device->GetId(&value);
    if (FAILED(hr) || !value) return {};
    std::wstring copy(value);
    CoTaskMemFree(value);
    return utf8(copy);
}

static std::string propertyString(IPropertyStore* store, REFPROPERTYKEY key) {
    PROPVARIANT value;
    PropVariantInit(&value);
    std::string result;
    if (SUCCEEDED(store->GetValue(key, &value))) {
        PWSTR text = nullptr;
        if (SUCCEEDED(PropVariantToStringAlloc(value, &text)) && text) {
            result = utf8(text);
            CoTaskMemFree(text);
        }
    }
    PropVariantClear(&value);
    return result;
}

static std::optional<uint32_t> propertyUint(IPropertyStore* store, REFPROPERTYKEY key) {
    PROPVARIANT value;
    PropVariantInit(&value);
    std::optional<uint32_t> result;
    if (SUCCEEDED(store->GetValue(key, &value))) {
        ULONG number = 0;
        if (SUCCEEDED(PropVariantToUInt32(value, &number))) result = static_cast<uint32_t>(number);
    }
    PropVariantClear(&value);
    return result;
}

static std::string bluetoothProfile(const std::string& friendly, const std::string& instanceId, EDataFlow flow) {
    std::string lower = friendly + " " + instanceId;
    std::transform(lower.begin(), lower.end(), lower.begin(), [](unsigned char c) { return static_cast<char>(std::tolower(c)); });
    const bool bluetooth = lower.find("bluetooth") != std::string::npos || lower.find("bthenum") != std::string::npos ||
        lower.find("hands-free") != std::string::npos || friendly.find("蓝牙") != std::string::npos;
    const bool handsFree = lower.find("hands-free") != std::string::npos || lower.find("handsfree") != std::string::npos ||
        friendly.find("免提") != std::string::npos;
    if (handsFree) return "hfp_hsp_candidate";
    if (bluetooth && flow == eRender) return "a2dp_candidate";
    if (bluetooth && flow == eCapture) return "bluetooth_capture_candidate";
    return "not_bluetooth";
}

struct FormatInfo {
    uint16_t formatTag = 0;
    uint16_t channels = 0;
    uint32_t sampleRate = 0;
    uint32_t avgBytesPerSec = 0;
    uint16_t blockAlign = 0;
    uint16_t bitsPerSample = 0;
    uint16_t validBitsPerSample = 0;
    uint32_t channelMask = 0;
    GUID subFormat{};
    bool extensible = false;
};

static FormatInfo formatInfo(const WAVEFORMATEX* fmt) {
    FormatInfo info;
    if (!fmt) return info;
    info.formatTag = fmt->wFormatTag;
    info.channels = fmt->nChannels;
    info.sampleRate = fmt->nSamplesPerSec;
    info.avgBytesPerSec = fmt->nAvgBytesPerSec;
    info.blockAlign = fmt->nBlockAlign;
    info.bitsPerSample = fmt->wBitsPerSample;
    if (fmt->wFormatTag == WAVE_FORMAT_EXTENSIBLE && fmt->cbSize >= 22) {
        const auto* ext = reinterpret_cast<const WAVEFORMATEXTENSIBLE*>(fmt);
        info.extensible = true;
        info.validBitsPerSample = ext->Samples.wValidBitsPerSample;
        info.channelMask = ext->dwChannelMask;
        info.subFormat = ext->SubFormat;
    } else {
        info.validBitsPerSample = fmt->wBitsPerSample;
        info.subFormat = fmt->wFormatTag == WAVE_FORMAT_IEEE_FLOAT ? KSDATAFORMAT_SUBTYPE_IEEE_FLOAT : KSDATAFORMAT_SUBTYPE_PCM;
    }
    return info;
}

static std::string sampleKind(const FormatInfo& info) {
    const bool floating = info.formatTag == WAVE_FORMAT_IEEE_FLOAT ||
        (info.extensible && IsEqualGUID(info.subFormat, KSDATAFORMAT_SUBTYPE_IEEE_FLOAT));
    const bool pcm = info.formatTag == WAVE_FORMAT_PCM ||
        (info.extensible && IsEqualGUID(info.subFormat, KSDATAFORMAT_SUBTYPE_PCM));
    if (floating) return "float";
    if (pcm) return "pcm_integer";
    return "unknown";
}

static std::string formatJson(const FormatInfo& info) {
    std::ostringstream out;
    out << "{\n"
        << "      \"format_tag\": " << info.formatTag << ",\n"
        << "      \"sample_kind\": " << quote(sampleKind(info)) << ",\n"
        << "      \"sample_rate_hz\": " << info.sampleRate << ",\n"
        << "      \"channels\": " << info.channels << ",\n"
        << "      \"bits_per_sample\": " << info.bitsPerSample << ",\n"
        << "      \"valid_bits_per_sample\": " << info.validBitsPerSample << ",\n"
        << "      \"block_align_bytes\": " << info.blockAlign << ",\n"
        << "      \"avg_bytes_per_second\": " << info.avgBytesPerSec << ",\n"
        << "      \"channel_mask\": " << info.channelMask << ",\n"
        << "      \"sub_format\": " << quote(guidString(info.subFormat)) << "\n"
        << "    }";
    return out.str();
}

static std::optional<FormatInfo> endpointMixFormat(IMMDevice* device, HRESULT& failure) {
    ComPtr<IAudioClient> client;
    failure = device->Activate(__uuidof(IAudioClient), CLSCTX_ALL, nullptr, reinterpret_cast<void**>(client.put()));
    if (FAILED(failure)) return std::nullopt;
    WAVEFORMATEX* fmt = nullptr;
    failure = client->GetMixFormat(&fmt);
    if (FAILED(failure) || !fmt) return std::nullopt;
    FormatInfo info = formatInfo(fmt);
    CoTaskMemFree(fmt);
    return info;
}

static ComPtr<IMMDeviceEnumerator> createEnumerator(HRESULT& hr) {
    ComPtr<IMMDeviceEnumerator> enumerator;
    hr = CoCreateInstance(__uuidof(MMDeviceEnumerator), nullptr, CLSCTX_ALL, __uuidof(IMMDeviceEnumerator),
        reinterpret_cast<void**>(enumerator.put()));
    return enumerator;
}

static ComPtr<IMMDevice> getDeviceById(const std::string& id, HRESULT& hr) {
    ComPtr<IMMDeviceEnumerator> enumerator = createEnumerator(hr);
    if (FAILED(hr)) return {};
    ComPtr<IMMDevice> device;
    std::wstring wide = widen(id);
    hr = enumerator->GetDevice(wide.c_str(), device.put());
    return device;
}

static std::map<std::pair<EDataFlow, ERole>, std::string> defaultEndpointIds(IMMDeviceEnumerator* enumerator) {
    std::map<std::pair<EDataFlow, ERole>, std::string> defaults;
    for (EDataFlow flow : {eRender, eCapture}) {
        for (ERole role : {eConsole, eMultimedia, eCommunications}) {
            ComPtr<IMMDevice> device;
            if (SUCCEEDED(enumerator->GetDefaultAudioEndpoint(flow, role, device.put()))) {
                defaults[{flow, role}] = endpointId(device.get());
            }
        }
    }
    return defaults;
}

static int enumerateEndpoints(const fs::path& output) {
    CoInit com;
    if (FAILED(com.hr) && com.hr != RPC_E_CHANGED_MODE) return 2;
    HRESULT hr = S_OK;
    ComPtr<IMMDeviceEnumerator> enumerator = createEnumerator(hr);
    if (FAILED(hr)) return 3;
    auto defaults = defaultEndpointIds(enumerator.get());
    std::ofstream out(output, std::ios::binary);
    if (!out) return 4;
    out << "{\n  \"schema_version\": 1,\n  \"probe\": \"meetily-d10a-wasapi-probe\",\n"
        << "  \"captured_at_utc\": " << quote(isoUtcNow()) << ",\n"
        << "  \"qpc_frequency_hz\": " << qpcFrequency() << ",\n"
        << "  \"defaults\": {\n";
    bool firstDefault = true;
    for (const auto& item : defaults) {
        if (!firstDefault) out << ",\n";
        firstDefault = false;
        const std::string flow = item.first.first == eRender ? "render" : "capture";
        out << "    " << quote(flow + "_" + roleName(item.first.second)) << ": " << quote(item.second);
    }
    out << "\n  },\n  \"endpoints\": [\n";
    bool firstEndpoint = true;
    for (EDataFlow flow : {eRender, eCapture}) {
        ComPtr<IMMDeviceCollection> collection;
        hr = enumerator->EnumAudioEndpoints(flow, DEVICE_STATEMASK_ALL, collection.put());
        if (FAILED(hr)) continue;
        UINT count = 0;
        collection->GetCount(&count);
        for (UINT index = 0; index < count; ++index) {
            ComPtr<IMMDevice> device;
            if (FAILED(collection->Item(index, device.put()))) continue;
            DWORD state = 0;
            device->GetState(&state);
            ComPtr<IPropertyStore> store;
            std::string friendly;
            std::string interfaceFriendly;
            std::string instance;
            std::optional<uint32_t> formFactor;
            if (SUCCEEDED(device->OpenPropertyStore(STGM_READ, store.put()))) {
                friendly = propertyString(store.get(), PKEY_Device_FriendlyName);
                interfaceFriendly = propertyString(store.get(), PKEY_DeviceInterface_FriendlyName);
                instance = propertyString(store.get(), PKEY_Device_InstanceId);
                formFactor = propertyUint(store.get(), PKEY_AudioEndpoint_FormFactor);
            }
            std::string id = endpointId(device.get());
            HRESULT mixHr = S_OK;
            auto mix = endpointMixFormat(device.get(), mixHr);
            if (!firstEndpoint) out << ",\n";
            firstEndpoint = false;
            out << "    {\n"
                << "      \"flow\": " << quote(flow == eRender ? "render" : "capture") << ",\n"
                << "      \"id\": " << quote(id) << ",\n"
                << "      \"state\": " << quote(stateName(state)) << ",\n"
                << "      \"state_mask\": " << state << ",\n"
                << "      \"friendly_name\": " << quote(friendly) << ",\n"
                << "      \"interface_friendly_name\": " << quote(interfaceFriendly) << ",\n"
                << "      \"pnp_instance_id\": " << quote(instance) << ",\n"
                << "      \"endpoint_form_factor\": ";
            if (formFactor) out << *formFactor; else out << "null";
            out << ",\n      \"bluetooth_profile\": " << quote(bluetoothProfile(friendly, instance, flow)) << ",\n"
                << "      \"roles\": [";
            bool firstRole = true;
            for (ERole role : {eConsole, eMultimedia, eCommunications}) {
                auto found = defaults.find({flow, role});
                if (found != defaults.end() && found->second == id) {
                    if (!firstRole) out << ", ";
                    firstRole = false;
                    out << quote(roleName(role));
                }
            }
            out << "],\n      \"mix_format_hresult\": " << quote(hrHex(mixHr)) << ",\n      \"mix_format\": ";
            if (mix) out << formatJson(*mix); else out << "null";
            out << "\n    }";
        }
    }
    out << "\n  ]\n}\n";
    return 0;
}

static std::string sha256(const std::vector<uint8_t>& bytes) {
    BCRYPT_ALG_HANDLE algorithm = nullptr;
    BCRYPT_HASH_HANDLE hash = nullptr;
    DWORD objectLength = 0, hashLength = 0, used = 0;
    std::vector<uint8_t> object;
    std::vector<uint8_t> digest;
    if (BCryptOpenAlgorithmProvider(&algorithm, BCRYPT_SHA256_ALGORITHM, nullptr, 0) < 0) return {};
    if (BCryptGetProperty(algorithm, BCRYPT_OBJECT_LENGTH, reinterpret_cast<PUCHAR>(&objectLength), sizeof(objectLength), &used, 0) < 0) goto cleanup;
    if (BCryptGetProperty(algorithm, BCRYPT_HASH_LENGTH, reinterpret_cast<PUCHAR>(&hashLength), sizeof(hashLength), &used, 0) < 0) goto cleanup;
    object.resize(objectLength);
    digest.resize(hashLength);
    if (BCryptCreateHash(algorithm, &hash, object.data(), objectLength, nullptr, 0, 0) < 0) goto cleanup;
    if (!bytes.empty() && BCryptHashData(hash, const_cast<PUCHAR>(bytes.data()), static_cast<ULONG>(bytes.size()), 0) < 0) goto cleanup;
    if (BCryptFinishHash(hash, digest.data(), hashLength, 0) < 0) goto cleanup;
    {
        std::ostringstream out;
        for (uint8_t value : digest) out << std::uppercase << std::hex << std::setw(2) << std::setfill('0') << static_cast<int>(value);
        if (hash) BCryptDestroyHash(hash);
        if (algorithm) BCryptCloseAlgorithmProvider(algorithm, 0);
        return out.str();
    }
cleanup:
    if (hash) BCryptDestroyHash(hash);
    if (algorithm) BCryptCloseAlgorithmProvider(algorithm, 0);
    return {};
}

static float readSample(const uint8_t* data, const FormatInfo& fmt, size_t sampleIndex) {
    const std::string kind = sampleKind(fmt);
    const uint8_t* sample = data + sampleIndex * (fmt.bitsPerSample / 8);
    if (kind == "float" && fmt.bitsPerSample == 32) {
        float value;
        memcpy(&value, sample, sizeof(value));
        return std::isfinite(value) ? std::clamp(value, -1.0f, 1.0f) : 0.0f;
    }
    if (kind == "float" && fmt.bitsPerSample == 64) {
        double value;
        memcpy(&value, sample, sizeof(value));
        return std::isfinite(value) ? static_cast<float>(std::clamp(value, -1.0, 1.0)) : 0.0f;
    }
    if (kind == "pcm_integer" && fmt.bitsPerSample == 16) {
        int16_t value;
        memcpy(&value, sample, sizeof(value));
        return static_cast<float>(value / 32768.0);
    }
    if (kind == "pcm_integer" && fmt.bitsPerSample == 24) {
        int32_t value = sample[0] | (sample[1] << 8) | (sample[2] << 16);
        if (value & 0x800000) value |= ~0xFFFFFF;
        return static_cast<float>(value / 8388608.0);
    }
    if (kind == "pcm_integer" && fmt.bitsPerSample == 32) {
        int32_t value;
        memcpy(&value, sample, sizeof(value));
        return static_cast<float>(value / 2147483648.0);
    }
    if (kind == "pcm_integer" && fmt.bitsPerSample == 8) return (static_cast<int>(*sample) - 128) / 128.0f;
    return 0.0f;
}

static void writeSample(uint8_t* data, const FormatInfo& fmt, size_t sampleIndex, float input) {
    float value = std::clamp(input, -1.0f, 1.0f);
    uint8_t* sample = data + sampleIndex * (fmt.bitsPerSample / 8);
    const std::string kind = sampleKind(fmt);
    if (kind == "float" && fmt.bitsPerSample == 32) { memcpy(sample, &value, sizeof(value)); return; }
    if (kind == "float" && fmt.bitsPerSample == 64) { double d = value; memcpy(sample, &d, sizeof(d)); return; }
    if (kind == "pcm_integer" && fmt.bitsPerSample == 16) { int16_t v = static_cast<int16_t>(std::lrint(value * 32767.0f)); memcpy(sample, &v, sizeof(v)); return; }
    if (kind == "pcm_integer" && fmt.bitsPerSample == 24) {
        int32_t v = static_cast<int32_t>(std::lrint(value * 8388607.0f)); sample[0] = v & 0xFF; sample[1] = (v >> 8) & 0xFF; sample[2] = (v >> 16) & 0xFF; return;
    }
    if (kind == "pcm_integer" && fmt.bitsPerSample == 32) { int32_t v = static_cast<int32_t>(std::llround(value * 2147483647.0)); memcpy(sample, &v, sizeof(v)); return; }
    if (kind == "pcm_integer" && fmt.bitsPerSample == 8) { *sample = static_cast<uint8_t>(std::lrint((value + 1.0f) * 127.5f)); return; }
}

struct WavData {
    FormatInfo format;
    std::vector<uint8_t> pcm;
};

static bool loadWav(const fs::path& path, WavData& wav, std::string& error) {
    std::ifstream in(path, std::ios::binary);
    if (!in) { error = "open_wav_failed"; return false; }
    char riff[4], wave[4];
    uint32_t size = 0;
    in.read(riff, 4); in.read(reinterpret_cast<char*>(&size), 4); in.read(wave, 4);
    if (!in || memcmp(riff, "RIFF", 4) != 0 || memcmp(wave, "WAVE", 4) != 0) { error = "invalid_riff_wave"; return false; }
    bool haveFmt = false, haveData = false;
    while (in && !(haveFmt && haveData)) {
        char id[4]; uint32_t chunkSize = 0;
        in.read(id, 4); in.read(reinterpret_cast<char*>(&chunkSize), 4);
        if (!in) break;
        if (memcmp(id, "fmt ", 4) == 0) {
            std::vector<uint8_t> bytes(chunkSize);
            in.read(reinterpret_cast<char*>(bytes.data()), chunkSize);
            if (chunkSize < 16) { error = "short_fmt"; return false; }
            auto* fmt = reinterpret_cast<WAVEFORMATEX*>(bytes.data());
            wav.format = formatInfo(fmt);
            haveFmt = true;
        } else if (memcmp(id, "data", 4) == 0) {
            wav.pcm.resize(chunkSize);
            in.read(reinterpret_cast<char*>(wav.pcm.data()), chunkSize);
            haveData = true;
        } else {
            in.seekg(chunkSize, std::ios::cur);
        }
        if (chunkSize & 1) in.seekg(1, std::ios::cur);
    }
    if (!haveFmt || !haveData || wav.format.channels == 0 || wav.format.blockAlign == 0) { error = "missing_fmt_or_data"; return false; }
    return true;
}

static std::vector<float> toMono(const std::vector<uint8_t>& bytes, const FormatInfo& fmt) {
    std::vector<float> mono;
    if (fmt.blockAlign == 0 || fmt.channels == 0 || fmt.bitsPerSample == 0) return mono;
    size_t frames = bytes.size() / fmt.blockAlign;
    mono.resize(frames);
    for (size_t frame = 0; frame < frames; ++frame) {
        double sum = 0.0;
        for (size_t channel = 0; channel < fmt.channels; ++channel) {
            sum += readSample(bytes.data() + frame * fmt.blockAlign, fmt, channel);
        }
        mono[frame] = static_cast<float>(sum / fmt.channels);
    }
    return mono;
}

static std::vector<float> resampleMono(const std::vector<float>& input, uint32_t sourceRate, uint32_t targetRate) {
    if (input.empty() || sourceRate == 0 || targetRate == 0) return {};
    if (sourceRate == targetRate) return input;
    size_t outputFrames = static_cast<size_t>(std::llround(static_cast<long double>(input.size()) * targetRate / sourceRate));
    std::vector<float> output(outputFrames);
    for (size_t i = 0; i < outputFrames; ++i) {
        long double position = static_cast<long double>(i) * sourceRate / targetRate;
        size_t left = std::min(static_cast<size_t>(position), input.size() - 1);
        size_t right = std::min(left + 1, input.size() - 1);
        float fraction = static_cast<float>(position - left);
        output[i] = input[left] + (input[right] - input[left]) * fraction;
    }
    return output;
}

static std::vector<uint8_t> floatBytes(const std::vector<float>& values) {
    std::vector<uint8_t> bytes(values.size() * sizeof(float));
    if (!values.empty()) memcpy(bytes.data(), values.data(), bytes.size());
    return bytes;
}

struct SecondBucket { uint64_t callbacks = 0; uint64_t frames = 0; };

struct RouteResult {
    std::string route;
    std::string endpointId;
    bool requested = false;
    bool streamCreated = false;
    bool streamStarted = false;
    HRESULT createHr = E_FAIL;
    HRESULT initializeHr = E_FAIL;
    HRESULT startHr = E_FAIL;
    std::string error;
    FormatInfo format;
    uint64_t deviceEpoch = 1;
    uint64_t streamStartedQpcNs = 0;
    uint64_t firstFrameQpcNs = 0;
    uint64_t lastFrameQpcNs = 0;
    uint64_t callbacks = 0;
    uint64_t frames = 0;
    uint64_t allZeroFrames = 0;
    uint64_t silentFlagFrames = 0;
    uint64_t maxCallbackGapNs = 0;
    bool twoSecondsWithoutCallback = false;
    double rms = 0.0;
    double peak = 0.0;
    std::string preMixSha256;
    std::string engineBeforeSha256;
    uint64_t engineFrames = 0;
    std::vector<SecondBucket> buckets;
    std::vector<uint8_t> rawBytes;
    std::vector<uint8_t> engineBytes;
};

static RouteResult captureRoute(const std::string& route, const std::string& id, bool loopback, uint32_t durationMs) {
    RouteResult result;
    result.route = route;
    result.endpointId = id;
    result.requested = true;
    CoInit com;
    if (FAILED(com.hr) && com.hr != RPC_E_CHANGED_MODE) { result.error = "coinitialize:" + hrHex(com.hr); return result; }
    HRESULT hr = S_OK;
    ComPtr<IMMDevice> device = getDeviceById(id, hr);
    result.createHr = hr;
    if (FAILED(hr)) { result.error = "get_device:" + hrHex(hr); return result; }
    ComPtr<IAudioClient> client;
    hr = device->Activate(__uuidof(IAudioClient), CLSCTX_ALL, nullptr, reinterpret_cast<void**>(client.put()));
    result.createHr = hr;
    if (FAILED(hr)) { result.error = "activate_audio_client:" + hrHex(hr); return result; }
    result.streamCreated = true;
    WAVEFORMATEX* fmt = nullptr;
    hr = client->GetMixFormat(&fmt);
    if (FAILED(hr) || !fmt) { result.initializeHr = hr; result.error = "get_mix_format:" + hrHex(hr); return result; }
    result.format = formatInfo(fmt);
    DWORD flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK | (loopback ? AUDCLNT_STREAMFLAGS_LOOPBACK : 0);
    hr = client->Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 0, 0, fmt, nullptr);
    result.initializeHr = hr;
    CoTaskMemFree(fmt);
    if (FAILED(hr)) { result.error = "initialize_capture:" + hrHex(hr); return result; }
    HANDLE eventHandle = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    if (!eventHandle) { result.error = "create_event:" + std::to_string(GetLastError()); return result; }
    hr = client->SetEventHandle(eventHandle);
    if (FAILED(hr)) { CloseHandle(eventHandle); result.error = "set_event:" + hrHex(hr); return result; }
    ComPtr<IAudioCaptureClient> capture;
    hr = client->GetService(__uuidof(IAudioCaptureClient), reinterpret_cast<void**>(capture.put()));
    if (FAILED(hr)) { CloseHandle(eventHandle); result.error = "get_capture_service:" + hrHex(hr); return result; }
    hr = client->Start();
    const uint64_t startAfter = qpcNowNs();
    result.startHr = hr;
    if (FAILED(hr)) { CloseHandle(eventHandle); result.error = "start_capture:" + hrHex(hr); return result; }
    result.streamStarted = true;
    result.streamStartedQpcNs = startAfter;
    const uint64_t stopAt = startAfter + static_cast<uint64_t>(durationMs) * 1000000ULL;
    uint64_t priorFrameQpcNs = 0;
    while (qpcNowNs() < stopAt) {
        WaitForSingleObject(eventHandle, 100);
        UINT32 packetFrames = 0;
        hr = capture->GetNextPacketSize(&packetFrames);
        if (FAILED(hr)) { result.error = "get_next_packet:" + hrHex(hr); break; }
        while (packetFrames > 0) {
            BYTE* data = nullptr;
            UINT32 frames = 0;
            DWORD bufferFlags = 0;
            UINT64 devicePosition = 0;
            UINT64 qpcPosition100ns = 0;
            hr = capture->GetBuffer(&data, &frames, &bufferFlags, &devicePosition, &qpcPosition100ns);
            if (FAILED(hr)) { result.error = "get_buffer:" + hrHex(hr); break; }
            const uint64_t bufferQpcNs = qpcPosition100ns * 100ULL;
            if (result.firstFrameQpcNs == 0) result.firstFrameQpcNs = bufferQpcNs;
            result.lastFrameQpcNs = bufferQpcNs;
            if (priorFrameQpcNs != 0 && bufferQpcNs >= priorFrameQpcNs) result.maxCallbackGapNs = std::max(result.maxCallbackGapNs, bufferQpcNs - priorFrameQpcNs);
            priorFrameQpcNs = bufferQpcNs;
            result.callbacks++;
            result.frames += frames;
            size_t bucketIndex = bufferQpcNs >= startAfter ? static_cast<size_t>((bufferQpcNs - startAfter) / 1000000000ULL) : 0;
            if (result.buckets.size() <= bucketIndex) result.buckets.resize(bucketIndex + 1);
            result.buckets[bucketIndex].callbacks++;
            result.buckets[bucketIndex].frames += frames;
            const size_t byteCount = static_cast<size_t>(frames) * result.format.blockAlign;
            const bool silent = (bufferFlags & AUDCLNT_BUFFERFLAGS_SILENT) != 0 || data == nullptr;
            const size_t base = result.rawBytes.size();
            result.rawBytes.resize(base + byteCount, 0);
            if (!silent && byteCount > 0) memcpy(result.rawBytes.data() + base, data, byteCount);
            if (silent) {
                result.silentFlagFrames += frames;
                result.allZeroFrames += frames;
            } else {
                for (UINT32 frame = 0; frame < frames; ++frame) {
                    bool allZero = true;
                    for (uint16_t channel = 0; channel < result.format.channels; ++channel) {
                        if (std::fabs(readSample(data + static_cast<size_t>(frame) * result.format.blockAlign, result.format, channel)) > 1e-12f) {
                            allZero = false;
                            break;
                        }
                    }
                    if (allZero) result.allZeroFrames++;
                }
            }
            capture->ReleaseBuffer(frames);
            hr = capture->GetNextPacketSize(&packetFrames);
            if (FAILED(hr)) { result.error = "get_next_packet_after_release:" + hrHex(hr); break; }
        }
        if (FAILED(hr)) break;
    }
    client->Stop();
    CloseHandle(eventHandle);
    if (result.firstFrameQpcNs == 0) {
        result.maxCallbackGapNs = qpcNowNs() - startAfter;
    } else {
        result.maxCallbackGapNs = std::max(result.maxCallbackGapNs, result.firstFrameQpcNs > startAfter ? result.firstFrameQpcNs - startAfter : 0ULL);
        result.maxCallbackGapNs = std::max(result.maxCallbackGapNs, qpcNowNs() > result.lastFrameQpcNs ? qpcNowNs() - result.lastFrameQpcNs : 0ULL);
    }
    result.twoSecondsWithoutCallback = result.maxCallbackGapNs >= 2000000000ULL;
    result.preMixSha256 = sha256(result.rawBytes);
    std::vector<float> mono = toMono(result.rawBytes, result.format);
    std::vector<float> engine = resampleMono(mono, result.format.sampleRate, 48000);
    long double squareSum = 0.0L;
    for (float value : mono) {
        squareSum += static_cast<long double>(value) * value;
        result.peak = std::max(result.peak, static_cast<double>(std::fabs(value)));
    }
    result.rms = mono.empty() ? 0.0 : std::sqrt(static_cast<double>(squareSum / mono.size()));
    result.engineFrames = engine.size();
    result.engineBytes = floatBytes(engine);
    result.engineBeforeSha256 = sha256(result.engineBytes);
    return result;
}

static HRESULT getEndpointVolume(const std::string& id, ComPtr<IAudioEndpointVolume>& volume) {
    HRESULT hr = S_OK;
    ComPtr<IMMDevice> device = getDeviceById(id, hr);
    if (FAILED(hr)) return hr;
    return device->Activate(__uuidof(IAudioEndpointVolume), CLSCTX_ALL, nullptr, reinterpret_cast<void**>(volume.put()));
}

struct VolumeState {
    bool available = false;
    bool beforeMute = false;
    bool targetMute = false;
    bool observedTargetMute = false;
    bool restoredMute = false;
    float masterScalar = 0.0f;
    HRESULT activateHr = E_FAIL;
    HRESULT setHr = E_FAIL;
    HRESULT restoreHr = E_FAIL;
};

static HRESULT renderWav(const std::string& endpoint, const WavData& wav, uint64_t& startedQpcNs, uint64_t& finishedQpcNs, FormatInfo& outputFormat) {
    CoInit com;
    if (FAILED(com.hr) && com.hr != RPC_E_CHANGED_MODE) return com.hr;
    HRESULT hr = S_OK;
    ComPtr<IMMDevice> device = getDeviceById(endpoint, hr);
    if (FAILED(hr)) return hr;
    ComPtr<IAudioClient> client;
    hr = device->Activate(__uuidof(IAudioClient), CLSCTX_ALL, nullptr, reinterpret_cast<void**>(client.put()));
    if (FAILED(hr)) return hr;
    WAVEFORMATEX* fmt = nullptr;
    hr = client->GetMixFormat(&fmt);
    if (FAILED(hr) || !fmt) return hr;
    outputFormat = formatInfo(fmt);
    hr = client->Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 0, 0, fmt, nullptr);
    CoTaskMemFree(fmt);
    if (FAILED(hr)) return hr;
    UINT32 bufferFrames = 0;
    hr = client->GetBufferSize(&bufferFrames);
    if (FAILED(hr)) return hr;
    ComPtr<IAudioRenderClient> render;
    hr = client->GetService(__uuidof(IAudioRenderClient), reinterpret_cast<void**>(render.put()));
    if (FAILED(hr)) return hr;
    std::vector<float> source = toMono(wav.pcm, wav.format);
    const size_t totalOutputFrames = static_cast<size_t>(std::llround(static_cast<long double>(source.size()) * outputFormat.sampleRate / wav.format.sampleRate));
    size_t writtenFrames = 0;
    hr = client->Start();
    startedQpcNs = qpcNowNs();
    if (FAILED(hr)) return hr;
    while (writtenFrames < totalOutputFrames) {
        UINT32 padding = 0;
        hr = client->GetCurrentPadding(&padding);
        if (FAILED(hr)) break;
        UINT32 available = bufferFrames > padding ? bufferFrames - padding : 0;
        if (available == 0) { Sleep(5); continue; }
        UINT32 framesToWrite = static_cast<UINT32>(std::min<size_t>(available, totalOutputFrames - writtenFrames));
        BYTE* buffer = nullptr;
        hr = render->GetBuffer(framesToWrite, &buffer);
        if (FAILED(hr)) break;
        memset(buffer, 0, static_cast<size_t>(framesToWrite) * outputFormat.blockAlign);
        for (UINT32 frame = 0; frame < framesToWrite; ++frame) {
            const size_t outputFrame = writtenFrames + frame;
            long double sourcePosition = static_cast<long double>(outputFrame) * wav.format.sampleRate / outputFormat.sampleRate;
            size_t left = std::min(static_cast<size_t>(sourcePosition), source.size() - 1);
            size_t right = std::min(left + 1, source.size() - 1);
            float fraction = static_cast<float>(sourcePosition - left);
            float value = source[left] + (source[right] - source[left]) * fraction;
            for (uint16_t channel = 0; channel < outputFormat.channels; ++channel) {
                writeSample(buffer + static_cast<size_t>(frame) * outputFormat.blockAlign, outputFormat, channel, value);
            }
        }
        hr = render->ReleaseBuffer(framesToWrite, 0);
        if (FAILED(hr)) break;
        writtenFrames += framesToWrite;
    }
    if (SUCCEEDED(hr)) {
        for (;;) {
            UINT32 padding = 0;
            if (FAILED(client->GetCurrentPadding(&padding)) || padding == 0) break;
            Sleep(5);
        }
    }
    finishedQpcNs = qpcNowNs();
    client->Stop();
    return hr;
}

static std::string routeJson(const RouteResult& route, const fs::path& relativePreMixPath, const fs::path& relativeEnginePath) {
    std::ostringstream out;
    out << "{\n"
        << "      \"route\": " << quote(route.route) << ",\n"
        << "      \"endpoint_id\": " << quote(route.endpointId) << ",\n"
        << "      \"requested\": " << (route.requested ? "true" : "false") << ",\n"
        << "      \"stream_created\": " << (route.streamCreated ? "true" : "false") << ",\n"
        << "      \"stream_started\": " << (route.streamStarted ? "true" : "false") << ",\n"
        << "      \"create_hresult\": " << quote(hrHex(route.createHr)) << ",\n"
        << "      \"initialize_hresult\": " << quote(hrHex(route.initializeHr)) << ",\n"
        << "      \"start_hresult\": " << quote(hrHex(route.startHr)) << ",\n"
        << "      \"error\": " << quote(route.error) << ",\n"
        << "      \"device_epoch\": " << route.deviceEpoch << ",\n"
        << "      \"actual_stream_format\": " << formatJson(route.format) << ",\n"
        << "      \"stream_started_qpc_ns\": " << route.streamStartedQpcNs << ",\n"
        << "      \"first_frame_qpc_ns\": " << route.firstFrameQpcNs << ",\n"
        << "      \"last_frame_qpc_ns\": " << route.lastFrameQpcNs << ",\n"
        << "      \"callback_count\": " << route.callbacks << ",\n"
        << "      \"callback_frames\": " << route.frames << ",\n"
        << "      \"all_zero_frames\": " << route.allZeroFrames << ",\n"
        << "      \"silent_flag_frames\": " << route.silentFlagFrames << ",\n"
        << "      \"rms\": " << std::setprecision(12) << route.rms << ",\n"
        << "      \"peak\": " << std::setprecision(12) << route.peak << ",\n"
        << "      \"max_callback_gap_ns\": " << route.maxCallbackGapNs << ",\n"
        << "      \"two_seconds_without_callback\": " << (route.twoSecondsWithoutCallback ? "true" : "false") << ",\n"
        << "      \"pre_mix_pcm_sha256\": " << quote(route.preMixSha256) << ",\n"
        << "      \"pre_mix_pcm_file\": " << quote(relativePreMixPath.generic_u8string()) << ",\n"
        << "      \"engine_before_pcm_spec\": \"48000Hz mono f32 little-endian; linear resample; QA evidence only\",\n"
        << "      \"engine_before_pcm_sha256\": " << quote(route.engineBeforeSha256) << ",\n"
        << "      \"engine_before_pcm_frames\": " << route.engineFrames << ",\n"
        << "      \"engine_before_pcm_file\": " << quote(relativeEnginePath.generic_u8string()) << ",\n"
        << "      \"per_second\": [";
    for (size_t i = 0; i < route.buckets.size(); ++i) {
        if (i) out << ',';
        out << "{\"second\":" << i << ",\"callbacks\":" << route.buckets[i].callbacks << ",\"frames\":" << route.buckets[i].frames << '}';
    }
    out << "]\n    }";
    return out.str();
}

static int captureScenario(const std::map<std::string, std::string>& args) {
    auto required = [&](const std::string& key) -> std::string {
        auto found = args.find(key);
        if (found == args.end()) throw std::runtime_error("missing argument --" + key);
        return found->second;
    };
    const std::string scenario = required("scenario");
    const std::string renderEndpoint = required("render-endpoint");
    const std::string microphoneEndpoint = args.count("microphone-endpoint") ? args.at("microphone-endpoint") : "";
    const fs::path audioPath = fs::u8path(required("audio"));
    const fs::path outputDir = fs::u8path(required("output-dir"));
    const uint32_t durationMs = static_cast<uint32_t>(std::stoul(args.count("duration-ms") ? args.at("duration-ms") : "27000"));
    const std::string renderMuteMode = args.count("render-mute") ? args.at("render-mute") : "preserve";
    fs::create_directories(outputDir);
    WavData wav;
    std::string wavError;
    if (!loadWav(audioPath, wav, wavError)) throw std::runtime_error(wavError);
    CoInit com;
    if (FAILED(com.hr) && com.hr != RPC_E_CHANGED_MODE) throw std::runtime_error("COM init failed");
    VolumeState volumeState;
    ComPtr<IAudioEndpointVolume> volume;
    volumeState.activateHr = getEndpointVolume(renderEndpoint, volume);
    if (SUCCEEDED(volumeState.activateHr)) {
        volumeState.available = true;
        BOOL beforeMute = FALSE;
        volume->GetMute(&beforeMute);
        volumeState.beforeMute = beforeMute != FALSE;
        volume->GetMasterVolumeLevelScalar(&volumeState.masterScalar);
        volumeState.targetMute = renderMuteMode == "muted" ? true : renderMuteMode == "unmuted" ? false : volumeState.beforeMute;
        volumeState.setHr = volume->SetMute(volumeState.targetMute ? TRUE : FALSE, nullptr);
        BOOL observed = FALSE;
        volume->GetMute(&observed);
        volumeState.observedTargetMute = observed != FALSE;
    }
    RouteResult systemResult;
    RouteResult microphoneResult;
    std::thread systemThread([&] { systemResult = captureRoute("system", renderEndpoint, true, durationMs); });
    std::optional<std::thread> microphoneThread;
    if (!microphoneEndpoint.empty() && microphoneEndpoint != "none") {
        microphoneThread.emplace([&] { microphoneResult = captureRoute("microphone", microphoneEndpoint, false, durationMs); });
    } else {
        microphoneResult.route = "microphone";
        microphoneResult.endpointId = microphoneEndpoint;
    }
    Sleep(750);
    uint64_t playbackStartedQpcNs = 0, playbackFinishedQpcNs = 0;
    FormatInfo playbackFormat;
    HRESULT playbackHr = renderWav(renderEndpoint, wav, playbackStartedQpcNs, playbackFinishedQpcNs, playbackFormat);
    systemThread.join();
    if (microphoneThread) microphoneThread->join();
    if (volumeState.available) {
        volumeState.restoreHr = volume->SetMute(volumeState.beforeMute ? TRUE : FALSE, nullptr);
        BOOL restored = FALSE;
        volume->GetMute(&restored);
        volumeState.restoredMute = restored != FALSE;
    }
    const fs::path systemPreMix = outputDir / "system.pre-mix.pcm";
    const fs::path systemEngine = outputDir / "system.engine-before.f32le.pcm";
    const fs::path microphonePreMix = outputDir / "microphone.pre-mix.pcm";
    const fs::path microphoneEngine = outputDir / "microphone.engine-before.f32le.pcm";
    if (!systemResult.rawBytes.empty()) {
        std::ofstream file(systemPreMix, std::ios::binary);
        file.write(reinterpret_cast<const char*>(systemResult.rawBytes.data()), static_cast<std::streamsize>(systemResult.rawBytes.size()));
    }
    if (!systemResult.engineBytes.empty()) {
        std::ofstream file(systemEngine, std::ios::binary);
        file.write(reinterpret_cast<const char*>(systemResult.engineBytes.data()), static_cast<std::streamsize>(systemResult.engineBytes.size()));
    }
    if (!microphoneResult.rawBytes.empty()) {
        std::ofstream file(microphonePreMix, std::ios::binary);
        file.write(reinterpret_cast<const char*>(microphoneResult.rawBytes.data()), static_cast<std::streamsize>(microphoneResult.rawBytes.size()));
    }
    if (!microphoneResult.engineBytes.empty()) {
        std::ofstream file(microphoneEngine, std::ios::binary);
        file.write(reinterpret_cast<const char*>(microphoneResult.engineBytes.data()), static_cast<std::streamsize>(microphoneResult.engineBytes.size()));
    }
    const std::string muteBehavior = systemResult.callbacks == 0 || systemResult.frames == 0 ? "no_callback" :
        (systemResult.peak > 1e-12 ? "audible_frames" : "silent_frames");
    const fs::path resultPath = outputDir / "result.json";
    std::ofstream out(resultPath, std::ios::binary);
    out << "{\n"
        << "  \"schema_version\": 1,\n"
        << "  \"scenario_id\": " << quote(scenario) << ",\n"
        << "  \"captured_at_utc\": " << quote(isoUtcNow()) << ",\n"
        << "  \"qpc_frequency_hz\": " << qpcFrequency() << ",\n"
        << "  \"qpc_source\": \"IAudioCaptureClient::GetBuffer qpcPosition converted from 100ns units to ns\",\n"
        << "  \"device_epoch\": 1,\n"
        << "  \"render_endpoint_id\": " << quote(renderEndpoint) << ",\n"
        << "  \"microphone_endpoint_id\": " << quote(microphoneEndpoint) << ",\n"
        << "  \"microphone_stream_started_count\": " << (microphoneResult.streamStarted ? 1 : 0) << ",\n"
        << "  \"render_mute\": {\"mode\":" << quote(renderMuteMode) << ",\"available\":" << (volumeState.available ? "true" : "false")
        << ",\"before\":" << (volumeState.beforeMute ? "true" : "false") << ",\"target\":" << (volumeState.targetMute ? "true" : "false")
        << ",\"observed_target\":" << (volumeState.observedTargetMute ? "true" : "false") << ",\"restored\":" << (volumeState.restoredMute ? "true" : "false")
        << ",\"master_scalar\":" << volumeState.masterScalar << ",\"activate_hresult\":" << quote(hrHex(volumeState.activateHr))
        << ",\"set_hresult\":" << quote(hrHex(volumeState.setHr)) << ",\"restore_hresult\":" << quote(hrHex(volumeState.restoreHr)) << "},\n"
        << "  \"playback\": {\"hresult\":" << quote(hrHex(playbackHr)) << ",\"started_qpc_ns\":" << playbackStartedQpcNs
        << ",\"finished_qpc_ns\":" << playbackFinishedQpcNs << ",\"actual_stream_format\":" << formatJson(playbackFormat) << "},\n"
        << "  \"routes\": [\n"
        << routeJson(systemResult, systemResult.rawBytes.empty() ? fs::path() : fs::path("system.pre-mix.pcm"), systemResult.engineBytes.empty() ? fs::path() : fs::path("system.engine-before.f32le.pcm")) << ",\n"
        << routeJson(microphoneResult, microphoneResult.rawBytes.empty() ? fs::path() : fs::path("microphone.pre-mix.pcm"), microphoneResult.engineBytes.empty() ? fs::path() : fs::path("microphone.engine-before.f32le.pcm")) << "\n  ],\n"
        << "  \"driver_mute_behavior_observed\": " << quote(muteBehavior) << "\n}\n";
    const bool captureSetupOk = systemResult.streamStarted && SUCCEEDED(playbackHr);
    return captureSetupOk ? 0 : 5;
}

static int decodeScenario(const std::map<std::string, std::string>& args) {
    auto required = [&](const std::string& key) -> std::string {
        auto found = args.find(key);
        if (found == args.end()) throw std::runtime_error("missing argument --" + key);
        return found->second;
    };
    const std::string runLabel = required("run-label");
    const std::string renderEndpoint = required("render-endpoint");
    const std::string renderMuteMode = required("render-mute");
    const fs::path audioPath = fs::u8path(required("audio"));
    const fs::path outputDir = fs::u8path(required("output-dir"));
    fs::create_directories(outputDir);
    WavData wav;
    std::string wavError;
    if (!loadWav(audioPath, wav, wavError)) throw std::runtime_error(wavError);
    CoInit com;
    if (FAILED(com.hr) && com.hr != RPC_E_CHANGED_MODE) throw std::runtime_error("COM init failed");
    VolumeState volumeState;
    ComPtr<IAudioEndpointVolume> volume;
    volumeState.activateHr = getEndpointVolume(renderEndpoint, volume);
    if (SUCCEEDED(volumeState.activateHr)) {
        volumeState.available = true;
        BOOL beforeMute = FALSE;
        volume->GetMute(&beforeMute);
        volumeState.beforeMute = beforeMute != FALSE;
        volume->GetMasterVolumeLevelScalar(&volumeState.masterScalar);
        volumeState.targetMute = renderMuteMode == "muted";
        volumeState.setHr = volume->SetMute(volumeState.targetMute ? TRUE : FALSE, nullptr);
        BOOL observed = FALSE;
        volume->GetMute(&observed);
        volumeState.observedTargetMute = observed != FALSE;
    }
    const uint64_t decodeStartedQpcNs = qpcNowNs();
    std::vector<float> mono = toMono(wav.pcm, wav.format);
    std::vector<float> engine = resampleMono(mono, wav.format.sampleRate, 48000);
    std::vector<uint8_t> engineBytes = floatBytes(engine);
    const std::string sourcePcmSha = sha256(wav.pcm);
    const std::string engineSha = sha256(engineBytes);
    const uint64_t decodeFinishedQpcNs = qpcNowNs();
    const fs::path enginePath = outputDir / "decoded.engine-before.f32le.pcm";
    {
        std::ofstream file(enginePath, std::ios::binary);
        file.write(reinterpret_cast<const char*>(engineBytes.data()), static_cast<std::streamsize>(engineBytes.size()));
    }
    if (volumeState.available) {
        volumeState.restoreHr = volume->SetMute(volumeState.beforeMute ? TRUE : FALSE, nullptr);
        BOOL restored = FALSE;
        volume->GetMute(&restored);
        volumeState.restoredMute = restored != FALSE;
    }
    std::ofstream out(outputDir / "result.json", std::ios::binary);
    out << "{\n"
        << "  \"schema_version\": 1,\n"
        << "  \"scenario_id\": \"D10A-07\",\n"
        << "  \"run_label\": " << quote(runLabel) << ",\n"
        << "  \"captured_at_utc\": " << quote(isoUtcNow()) << ",\n"
        << "  \"render_endpoint_id\": " << quote(renderEndpoint) << ",\n"
        << "  \"render_mute\": {\"mode\":" << quote(renderMuteMode) << ",\"available\":" << (volumeState.available ? "true" : "false")
        << ",\"before\":" << (volumeState.beforeMute ? "true" : "false") << ",\"target\":" << (volumeState.targetMute ? "true" : "false")
        << ",\"observed_target\":" << (volumeState.observedTargetMute ? "true" : "false") << ",\"restored\":" << (volumeState.restoredMute ? "true" : "false")
        << ",\"master_scalar\":" << volumeState.masterScalar << ",\"activate_hresult\":" << quote(hrHex(volumeState.activateHr))
        << ",\"set_hresult\":" << quote(hrHex(volumeState.setHr)) << ",\"restore_hresult\":" << quote(hrHex(volumeState.restoreHr)) << "},\n"
        << "  \"microphone_stream_started_count\": 0,\n"
        << "  \"microphone_callback_frames\": 0,\n"
        << "  \"decode_started_qpc_ns\": " << decodeStartedQpcNs << ",\n"
        << "  \"decode_finished_qpc_ns\": " << decodeFinishedQpcNs << ",\n"
        << "  \"source_format\": " << formatJson(wav.format) << ",\n"
        << "  \"source_pcm_sha256\": " << quote(sourcePcmSha) << ",\n"
        << "  \"engine_before_pcm_spec\": \"48000Hz mono f32 little-endian; linear resample; QA evidence only\",\n"
        << "  \"engine_before_pcm_sha256\": " << quote(engineSha) << ",\n"
        << "  \"engine_before_pcm_frames\": " << engine.size() << ",\n"
        << "  \"engine_before_pcm_file\": \"decoded.engine-before.f32le.pcm\",\n"
        << "  \"transcription_attempted\": false,\n"
        << "  \"transcription_reason\": \"D-10A probe verifies direct file decode only; no product or formal com.meetily.ai recording/import path was invoked\"\n"
        << "}\n";
    const bool observedCorrect = volumeState.available && volumeState.observedTargetMute == volumeState.targetMute && volumeState.restoredMute == volumeState.beforeMute;
    return observedCorrect ? 0 : 6;
}

static std::map<std::string, std::string> parseArgs(int argc, wchar_t** argv, int start) {
    std::map<std::string, std::string> values;
    for (int index = start; index < argc; ++index) {
        std::string key = utf8(argv[index]);
        if (key.rfind("--", 0) != 0 || index + 1 >= argc) throw std::runtime_error("expected --key value");
        values[key.substr(2)] = utf8(argv[++index]);
    }
    return values;
}

int wmain(int argc, wchar_t** argv) {
    SetConsoleOutputCP(CP_UTF8);
    SetConsoleCP(CP_UTF8);
    try {
        if (argc < 2) throw std::runtime_error("usage: d10a-wasapi-probe enumerate|capture ...");
        std::string mode = utf8(argv[1]);
        auto args = parseArgs(argc, argv, 2);
        if (mode == "enumerate") {
            auto found = args.find("output");
            if (found == args.end()) throw std::runtime_error("missing --output");
            return enumerateEndpoints(fs::u8path(found->second));
        }
        if (mode == "capture") return captureScenario(args);
        if (mode == "decode") return decodeScenario(args);
        throw std::runtime_error("unknown mode: " + mode);
    } catch (const std::exception& error) {
        std::cerr << "d10a-wasapi-probe error: " << error.what() << std::endl;
        return 64;
    }
}
