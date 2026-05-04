use transcoderr::plugins::discover;

#[test]
fn discovers_hello_plugin() {
    let dir = std::path::Path::new("tests/fixtures/plugins");
    let plugins = discover(dir).unwrap();
    let hello = plugins
        .iter()
        .find(|p| p.manifest.name == "hello")
        .expect("hello discovered");
    assert_eq!(hello.manifest.provides_steps, vec!["hello"]);
    assert_eq!(hello.schema["properties"]["greeting"]["type"], "string");
}
