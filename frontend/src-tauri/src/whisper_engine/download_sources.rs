// SHA-256 values published by ggerganov/whisper.cpp on Hugging Face.
// A mirror must serve exactly the same bytes as the upstream model.
pub(super) fn model_sha256(name: &str) -> Option<&'static str> {
    Some(match name {
        "tiny" => "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
        "base" => "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
        "small" => "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        "medium" => "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208",
        "large-v3" => "64d182b440b98d5203c4f9bd541544d84c605196c4f7b845dfa11fb23594d1e2",
        "large-v3-turbo" => "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
        "tiny-q5_1" => "818710568da3ca15689e31a743197b520007872ff9576237bda97bd1b469c3d7",
        "base-q5_1" => "422f1ae452ade6f30a004d7e5c6a43195e4433bc370bf23fac9cc591f01a8898",
        "small-q5_1" => "ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb",
        "medium-q5_0" => "19fea4b380c3a618ec4723c3eef2eb785ffba0d0538cf43f8f235e7b3b34220f",
        "large-v3-turbo-q5_0" => "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
        "large-v3-q5_0" => "d75795ecff3f83b5faa89d1900604ad8c780abd5739fae406de19f23ecd98ad1",
        _ => return None,
    })
}

pub(super) fn model_sources(name: &str) -> Vec<String> {
    // The older mirror predates Turbo and returns 404 for both Turbo variants.
    let mirror = match name {
        "large-v3-turbo" | "large-v3-turbo-q5_0" => "iceCream2025/whisper.cpp",
        _ => "cjc1887415157/whisper.cpp",
    };
    vec![
        format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{name}.bin"),
        format!("https://www.modelscope.cn/models/{mirror}/resolve/master/ggml-{name}.bin"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    // Opt-in because this checks external availability, not deterministic unit behavior.
    #[tokio::test]
    #[ignore]
    async fn all_catalog_mirrors_exist_and_match_upstream_hashes() {
        let client = reqwest::Client::builder()
            .user_agent(concat!("Meetily/", env!("CARGO_PKG_VERSION")))
            .timeout(std::time::Duration::from_secs(30)).build().unwrap();
        for model in crate::config::WHISPER_MODEL_CATALOG {
            let response = client.head(&model_sources(model.0)[1]).send().await.unwrap();
            assert!(response.status().is_success(), "{}: {}", model.0, response.status());
            let checksum = response.headers().get("x-linked-etag")
                .expect("mirror must publish the file hash").to_str().unwrap().trim_matches('"');
            assert_eq!(Some(checksum), model_sha256(model.0), "{} hash mismatch", model.0);
            eprintln!("{}: HTTP {} and upstream hash match", model.0, response.status());
        }
    }
}
