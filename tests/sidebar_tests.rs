use robinne::ui::sidebar::Panel;

#[test]
fn timeline_panel_title_uses_timeline_label() {
    assert_eq!(Panel::Timeline.title(), "时间线");
}
