use kikimimi_schema::{Event, COLUMNS, SCHEMA_VERSION};
#[test]
fn v1_wire_contract_round_trips_without_changing_columns_or_types() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../../contracts/kikimimi-v1.json")).unwrap();
    assert_eq!(fixture["schema"], SCHEMA_VERSION);
    assert_eq!(fixture["columns"], serde_json::json!(COLUMNS));
    for raw in fixture["events"].as_array().unwrap() {
        let event: Event = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(serde_json::to_value(event).unwrap(), *raw);
    }
}
