# whisper-rs-sys forwards CMAKE_* variables, so inject this after project().
# Avoid specializing native code for the CI runner; the x64 baseline retains AVX2.
set(GGML_NATIVE OFF CACHE BOOL "Build without host CPU specialization" FORCE)

# The local inference helper loads models from disk; it does not use llama.cpp's
# HTTP downloader. Avoid linking its optional HTTPS client to Homebrew OpenSSL.
# Meetily's Rust HTTPS clients keep their normal TLS verification.
if(APPLE)
  set(LLAMA_OPENSSL OFF CACHE BOOL "No optional HTTPS client in local inference helper" FORCE)
endif()
