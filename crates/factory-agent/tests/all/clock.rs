use factory_agent::clock::{iso_from_ms, iso_now, now_ms};

#[test]
fn iso_matches_date_to_iso_string() {
    assert_eq!(iso_from_ms(0), "1970-01-01T00:00:00.000Z");
    assert_eq!(iso_from_ms(951_782_400_000), "2000-02-29T00:00:00.000Z");
    assert_eq!(iso_from_ms(1_759_300_000_000), "2025-10-01T06:26:40.000Z");
    assert_eq!(iso_from_ms(1_789_661_414_272), "2026-09-17T16:10:14.272Z");
}

#[test]
fn now_is_after_the_crate_was_written() {
    assert!(now_ms() > 1_759_300_000_000);
    assert!(iso_now().starts_with("20"));
    assert_eq!(iso_now().len(), 24);
}
