use soundcraft_ui_egui::mix_window::strip_required_height;

#[test]
fn edit_channel_reserves_a_full_fader_below_all_eight_insert_and_send_slots() {
    let views = ["inserts_ae", "inserts_fj", "sends_ae", "sends_fj", "io"].map(String::from);
    // Controls end at 4 + 352 + 86 + 22 + 54 + 48 = 566 points.
    // The fader needs 160 points before the 64-point bottom readout.
    assert_eq!(strip_required_height(&views, 54.0), 790.0);
    assert_eq!(strip_required_height(&["all".into()], 110.0), 1038.0);
}
