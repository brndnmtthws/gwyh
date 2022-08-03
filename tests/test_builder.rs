#[test]
fn test_builder() {
    use gwyh::GwyhBuilder;
    let g = GwyhBuilder::new().build().unwrap();
    g.start().unwrap();
}
