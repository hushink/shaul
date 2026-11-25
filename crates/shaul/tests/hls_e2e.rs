#![cfg(feature = "hls")]

use shaul::Config;

#[test]
fn default_router_registers_the_hls_parser_and_backend() {
    let config = Config::default();
    let router = shaul::default_router(&config).unwrap();
    assert!(router.parsers().iter().any(|parser| parser.name() == "hls"));
    assert!(
        router
            .backends()
            .iter()
            .any(|backend| backend.name() == "hls")
    );
}
