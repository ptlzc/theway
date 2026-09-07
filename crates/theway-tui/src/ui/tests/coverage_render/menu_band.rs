use super::*;

#[tokio::test]
async fn render_menu_band_data_covers_picker_and_menu_levels() {
    use crate::model_picker::{ModelPickerState, PickerLevel};

    let (mut app, _rx) = test_app().await;
    let groups = vec![theway_transport::wire::ProviderGroup {
        provider: "p".into(),
        has_credential: true,
        models: vec![theway_transport::wire::ModelEntry {
            id: "m".into(),
            name: "M".into(),
        }],
    }];
    let picker = ModelPickerState::new(groups, Some(("p".into(), "m".into())), "high".into());
    app.model_picker = Some(picker);
    let data = app.menu_band_data().expect("provider menu");
    assert_eq!(data.active, 0);

    let mut picker = app.model_picker.take().unwrap();
    picker.level = PickerLevel::Models { provider_idx: 0 };
    app.model_picker = Some(picker);
    let data = app.menu_band_data().expect("model menu");
    assert_eq!(data.active, 1);

    let mut picker = app.model_picker.take().unwrap();
    picker.level = PickerLevel::Thinking {
        provider_idx: 0,
        model_idx: 0,
    };
    app.model_picker = Some(picker);
    let data = app.menu_band_data().expect("thinking menu");
    assert_eq!(data.active, 2);

    app.model_picker = None;
    app.graph_menu = Some(crate::ui::GraphMenuState {
        level: crate::ui::GraphMenuLevel::Position,
        cursor: 0,
        has_graphs: false,
    });
    assert!(app.menu_band_data().is_some());

    app.graph_menu = None;
    app.panel_menu = Some(crate::ui::PanelMenuState {
        level: crate::ui::PanelMenuLevel::Root,
        cursor: 0,
    });
    assert!(app.menu_band_data().is_some());
    app.panel_menu = Some(crate::ui::PanelMenuState {
        level: crate::ui::PanelMenuLevel::Toggle,
        cursor: 0,
    });
    assert!(app.menu_band_data().is_some());
}
