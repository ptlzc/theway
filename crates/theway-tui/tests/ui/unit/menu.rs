// Tests for the shared hierarchical-menu building blocks (`ui/menu.rs`):
// the canonical key mapping every menu handler shares and the band row
// budget used to size the inline band.

use super::*;

#[test]
fn map_menu_key_shares_bindings_between_menus() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let key = |code| KeyEvent::new(code, KeyModifiers::empty());
    let ctrl = |code| KeyEvent::new(code, KeyModifiers::CONTROL);

    // Cursor keys and vim keys map identically.
    for (code, want) in [
        (KeyCode::Up, MenuKey::Up),
        (KeyCode::Char('k'), MenuKey::Up),
        (KeyCode::Down, MenuKey::Down),
        (KeyCode::Char('j'), MenuKey::Down),
        (KeyCode::Left, MenuKey::Left),
        (KeyCode::Char('h'), MenuKey::Left),
        (KeyCode::Right, MenuKey::Right),
        (KeyCode::Char('l'), MenuKey::Right),
        (KeyCode::Enter, MenuKey::Enter),
        (KeyCode::Esc, MenuKey::Back),
    ] {
        assert_eq!(map_menu_key(&key(code)), want, "key {code:?}");
    }
    assert_eq!(map_menu_key(&ctrl(KeyCode::Char('c'))), MenuKey::Close);
    assert_eq!(map_menu_key(&key(KeyCode::Char('x'))), MenuKey::Other);
    // Plain `c` without Ctrl is not Close.
    assert_eq!(map_menu_key(&key(KeyCode::Char('c'))), MenuKey::Other);
}

#[test]
fn band_rows_is_breadcrumb_plus_windowed_choices() {
    // 0 choices degrade to one hint row under the breadcrumb.
    assert_eq!(menu::band_rows(0, 6), 2);
    // Choice counts below the cap size the band exactly.
    assert_eq!(menu::band_rows(2, 6), 3);
    assert_eq!(menu::band_rows(6, 6), 7);
    // Above the cap the band clamps.
    assert_eq!(menu::band_rows(20, 6), 7);
}

#[test]
fn panel_menu_state_items_match_levels() {
    use PanelMenuLevel as Level;
    assert_eq!(PanelMenuState { level: Level::Root, cursor: 0 }.items(), &PANEL_MENU_ROOT);
    assert_eq!(
        PanelMenuState { level: Level::Toggle, cursor: 0 }.items(),
        &PANEL_MENU_TOGGLE
    );
    assert_eq!(
        PanelMenuState { level: Level::Position, cursor: 0 }.items(),
        &PANEL_MENU_POSITION
    );
}
