use rua_skills_market::schema::StatValue;

#[test]
fn fixture_parsed_correctly() {
    let fixture = include_str!("../fixtures/aliyun_search.json");
    let body: serde_json::Value = serde_json::from_str(fixture).unwrap();
    let items: Vec<&serde_json::Map<String, serde_json::Value>> = body
        .get("data")
        .and_then(|v| v.as_array())
        .unwrap()
        .iter()
        .filter_map(|v| v.as_object())
        .collect();

    assert_eq!(items.len(), 3);

    let first = &items[0];
    assert_eq!(
        first.get("skillName").unwrap().as_str().unwrap(),
        "weather-query"
    );
    assert_eq!(
        first.get("displayName").unwrap().as_str().unwrap(),
        "Weather Query"
    );
    assert_eq!(first.get("installCount").unwrap().as_u64().unwrap(), 1234);
    assert_eq!(first.get("likeCount").unwrap().as_u64().unwrap(), 56);
    assert_eq!(
        first.get("categoryName").unwrap().as_str().unwrap(),
        "Tools"
    );
    assert_eq!(
        first.get("subCategoryName").unwrap().as_str().unwrap(),
        "Information"
    );

    let third = &items[2];
    assert_eq!(third.get("skillName").unwrap().as_str().unwrap(), "");
    assert_eq!(third.get("displayName").unwrap().as_str().unwrap(), "");

    assert_eq!(body.get("nextToken").unwrap().as_str().unwrap(), "abc123");
    assert_eq!(body.get("totalCount").unwrap().as_u64().unwrap(), 42);
}

#[test]
fn stat_value_serialization() {
    let int_val = StatValue::Int(42);
    let str_val = StatValue::Str("hello".to_string());
    assert_eq!(serde_json::to_string(&int_val).unwrap(), "42");
    assert_eq!(serde_json::to_string(&str_val).unwrap(), "\"hello\"");
}
