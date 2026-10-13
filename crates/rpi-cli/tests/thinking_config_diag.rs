//! 诊断：models.json → provider_to_models 后 thinking_level_map/compat 的实际内容。
#[test]
fn diag_linglong_thinking_map() {
    let raw = r#"{
      "api": "anthropic-messages",
      "baseUrl": "https://example.com",
      "authHeader": true,
      "models": [{
        "id": "test-model",
        "name": "Test",
        "reasoning": true,
        "contextWindow": 1000000,
        "maxTokens": 131072,
        "thinkingLevelMap": {"off": "none", "minimal": null, "low": "low", "medium": "medium", "high": "high", "xhigh": "xhigh", "max": "max"},
        "compat": {"forceAdaptiveThinking": true}
      }]
    }"#;
    let cfg: rpi_cli::config::ProviderConfig = serde_json::from_str(raw).unwrap();
    let models =
        rpi_cli::config::provider_to_models("linglong", &cfg).expect("provider should be accepted");
    let m = &models[0];
    let map = m.thinking_level_map.as_ref().expect("map parsed");
    println!("parsed map: {:?}", map);
    use rpi_ai::types::ThinkingLevel::*;
    assert_eq!(
        map.get(&Max),
        Some(&Some("max".to_string())),
        "max 键必须解析为 Some(max)"
    );
    assert_eq!(
        map.get(&Xhigh),
        Some(&Some("xhigh".to_string())),
        "xhigh 键必须解析"
    );
    assert!(
        matches!(
            m.compat,
            Some(rpi_ai::model::StreamingProtocolCompat::AnthropicMessages(_))
        ),
        "compat 必须是 AnthropicMessages"
    );
}
