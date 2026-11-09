use rua_sync::export::render_qr_svg;

#[test]
fn render_short_text() {
    let svg = render_qr_svg("hello world").unwrap();
    assert!(svg.contains("<svg"));
    assert!(svg.contains("</svg>"));
}

#[test]
fn render_armored_age_text() {
    let sample = "-----BEGIN AGE ENCRYPTED FILE-----\n\
                  YWdlLWVuY3J5cHRpb24ub3JnL3YxCi0+IHNjcnlwdCBhYmNk\n\
                  -----END AGE ENCRYPTED FILE-----";
    let svg = render_qr_svg(sample).unwrap();
    assert!(svg.contains("<svg"));
}

#[test]
fn empty_text_produces_valid_svg() {
    let svg = render_qr_svg("").unwrap();
    assert!(svg.contains("<svg"));
}
