#![cfg(feature = "dash")]

use shaul::Config;

#[test]
fn default_router_registers_the_dash_parser_and_backend() {
    let config = Config::default();
    let router = shaul::default_router(&config).unwrap();
    assert!(
        router
            .parsers()
            .iter()
            .any(|parser| parser.name() == "dash")
    );
    assert!(
        router
            .backends()
            .iter()
            .any(|backend| backend.name() == "dash")
    );
}
