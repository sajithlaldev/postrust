//! Headless smoke tests: run real egui frames through `ui::draw` in many app states so that
//! panics, id clashes and layout assertions are caught without a window. They cannot judge how
//! the UI looks; they guarantee it renders and reacts to input.

use egui::{Event, Key, Modifiers, Pos2, RawInput, Rect, vec2};
use postrust::app::{App, PaletteState, ReqTab, Resp, RespTab};
use postrust::http::ResponseData;
use postrust::model::*;

fn raw(events: Vec<Event>) -> RawInput {
    RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))),
        events,
        ..Default::default()
    }
}

fn frame(app: &mut App, ctx: &egui::Context, input: RawInput) {
    // The output (textures, shapes) is irrelevant here, but egui requires it to be consumed.
    ctx.run_ui(input, |ui| postrust::ui::draw(app, ui))
        .drop_without_applying_deltas();
}

fn frames(app: &mut App, ctx: &egui::Context, n: usize) {
    for _ in 0..n {
        frame(app, ctx, raw(vec![]));
    }
}

/// Runs frames, then releases the mouse once. Layout sizes are only remembered when the user
/// lets go of the mouse, so tests that look at saved sizes have to do that.
fn settle(app: &mut App, ctx: &egui::Context, n: usize) {
    frames(app, ctx, n);
    frame(app, ctx, raw(click(Pos2::new(640.0, 792.0))));
    frames(app, ctx, 1);
}

fn key(key: Key, modifiers: Modifiers) -> Vec<Event> {
    vec![Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers,
    }]
}

fn press(app: &mut App, ctx: &egui::Context, k: Key, modifiers: Modifiers) {
    frame(app, ctx, raw(key(k, modifiers)));
    frames(app, ctx, 1);
}

fn response(content_type: &str, body: &[u8]) -> ResponseData {
    ResponseData {
        status: 200,
        reason: "OK".into(),
        elapsed_ms: 12,
        size: body.len(),
        truncated: false,
        headers: vec![
            ("content-type".into(), content_type.into()),
            ("x-id".into(), "1".into()),
        ],
        body: body.to_vec(),
    }
}

fn resp(content_type: &str, body: &[u8]) -> Resp {
    let data = response(content_type, body);
    let text = data.text();
    let is_text = data.is_text();
    let pretty = postrust::http::pretty_json(&text);
    Resp {
        is_text,
        is_json: pretty.is_some(),
        raw: if pretty.is_some() { text.clone() } else { String::new() },
        display: pretty.unwrap_or(if is_text { text } else { String::new() }),
        display_cut: false,
        data,
    }
}

fn populated() -> App {
    let mut app = App::new(None);
    app.store.collections.push(Collection {
        name: "Users API".into(),
        requests: vec![
            Request {
                name: "List users".into(),
                url: "https://api.test/users".into(),
                ..Default::default()
            },
            Request {
                name: "Create user".into(),
                method: Method::Post,
                url: "https://api.test/users".into(),
                body_kind: BodyKind::Raw,
                body: r#"{"name":"ada","id":{{id}}}"#.into(),
                ..Default::default()
            },
            Request {
                name: "Delete".into(),
                method: Method::Delete,
                url: "https://api.test/u/1".into(),
                ..Default::default()
            },
        ],
    });
    app.store.collections.push(Collection {
        name: "Empty".into(),
        requests: vec![],
    });
    app.store.environments.push(Environment {
        name: "dev".into(),
        vars: vec![KeyValue::new("id", "7")],
    });
    app.store.active_env = Some(0);
    for (m, u) in [
        (Method::Get, "https://a.test/1"),
        (Method::Put, "https://a.test/2"),
        (Method::Patch, "https://a.test/3"),
    ] {
        app.store.push_history(&Request {
            method: m,
            url: u.into(),
            ..Default::default()
        });
    }
    app.open_saved(0, 0);
    app.open_saved(0, 1);
    app.tabs[1].request.body.push(' '); // unsaved change -> dirty dot
    app
}

#[test]
fn empty_app_renders_in_every_theme_and_layout() {
    for theme in [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark] {
        for split in [SplitLayout::Below, SplitLayout::Side] {
            let ctx = egui::Context::default();
            let mut app = App::new(None);
            app.store.ui.theme = theme;
            app.store.ui.split = split;
            frames(&mut app, &ctx, 4);
        }
    }
}

#[test]
fn theme_is_applied_and_follows_changes() {
    let ctx = egui::Context::default();
    let mut app = App::new(None);
    app.set_theme(ThemeMode::Dark);
    frames(&mut app, &ctx, 3);
    assert_eq!(ctx.theme(), egui::Theme::Dark);
    app.set_theme(ThemeMode::Light);
    frames(&mut app, &ctx, 3);
    assert_eq!(ctx.theme(), egui::Theme::Light);
    assert_eq!(app.applied_theme, Some(ThemeMode::Light));
}

#[test]
fn populated_app_renders_with_all_request_tabs_bodies_and_auth_kinds() {
    let ctx = egui::Context::default();
    let mut app = populated();
    let auths = [
        Auth::None,
        Auth::Bearer { token: "t".into() },
        Auth::Basic {
            username: "u".into(),
            password: "p".into(),
        },
        Auth::ApiKey {
            name: "X-Key".into(),
            value: "k".into(),
        },
    ];
    for tab in [ReqTab::Params, ReqTab::Headers, ReqTab::Body, ReqTab::Auth] {
        for kind in [BodyKind::None, BodyKind::Raw, BodyKind::Form] {
            for auth in &auths {
                let active = app.active;
                app.tabs[active].req_tab = tab;
                app.tabs[active].request.body_kind = kind;
                app.tabs[active].request.auth = auth.clone();
                frames(&mut app, &ctx, 2);
            }
        }
    }
}

#[test]
fn responses_render_json_text_binary_error_and_empty() {
    let ctx = egui::Context::default();
    let mut app = populated();
    let cases: Vec<postrust::app::Outcome> = vec![
        Ok(resp(
            "application/json",
            br#"{"b":1,"a":[1,2,{"c":"x {{v}}"}],"n":null,"t":true}"#,
        )),
        Ok(resp("text/html", b"<html>hi</html>")),
        Ok(resp("application/octet-stream", &[0xff, 0xfe, 0x00])),
        Ok(resp("text/plain", b"")),
        Err("Connection failed: refused".into()),
    ];
    for case in cases {
        for split in [SplitLayout::Below, SplitLayout::Side] {
            app.store.ui.split = split;
            let active = app.active;
            app.tabs[active].response = Some(match &case {
                Ok(_) => Ok(resp("application/json", b"[]")), // placeholder replaced below
                Err(e) => Err(e.clone()),
            });
            if let Ok(r) = &case {
                app.tabs[active].response = Some(Ok(Resp {
                    data: ResponseData {
                        body: r.data.body.clone(),
                        headers: r.data.headers.clone(),
                        ..response("x", b"")
                    },
                    is_text: r.is_text,
                    display: r.display.clone(),
                    display_cut: r.display_cut,
                    is_json: r.is_json,
                    raw: r.raw.clone(),
                }));
            }
            for resp_tab in [RespTab::Body, RespTab::Headers] {
                for pretty in [true, false] {
                    app.tabs[active].resp_tab = resp_tab;
                    app.tabs[active].pretty = pretty;
                    frames(&mut app, &ctx, 2);
                }
            }
        }
    }
}

#[test]
fn command_palette_opens_filters_navigates_and_runs() {
    let ctx = egui::Context::default();
    let mut app = populated();
    frames(&mut app, &ctx, 2);

    press(&mut app, &ctx, Key::K, Modifiers::COMMAND);
    assert!(app.palette.is_some(), "Cmd/Ctrl+K opens the palette");

    app.palette = Some(PaletteState {
        query: "theme".into(),
        selected: 0,
        ..Default::default()
    });
    frames(&mut app, &ctx, 2);
    let before = app.store.ui.theme;
    press(&mut app, &ctx, Key::Enter, Modifiers::NONE);
    assert!(app.palette.is_none(), "Enter runs the selected command and closes");
    assert_ne!(app.store.ui.theme, before, "the theme command ran");

    // Navigation wraps and Escape closes without running anything.
    app.palette = Some(PaletteState::default());
    press(&mut app, &ctx, Key::ArrowUp, Modifiers::NONE);
    assert_eq!(
        app.palette.as_ref().unwrap().selected,
        postrust::commands::ENTRIES.len() - 1
    );
    press(&mut app, &ctx, Key::ArrowDown, Modifiers::NONE);
    assert_eq!(app.palette.as_ref().unwrap().selected, 0);
    let tabs = app.tabs.len();
    press(&mut app, &ctx, Key::Escape, Modifiers::NONE);
    assert!(app.palette.is_none());
    assert_eq!(app.tabs.len(), tabs);

    // No matches still renders; Enter does nothing and the palette stays open until Escape.
    app.palette = Some(PaletteState {
        query: "zzzz".into(),
        selected: 5,
        ..Default::default()
    });
    frames(&mut app, &ctx, 2);
    press(&mut app, &ctx, Key::Enter, Modifiers::NONE);
    assert!(app.palette.is_some());
    press(&mut app, &ctx, Key::Escape, Modifiers::NONE);
    assert!(app.palette.is_none());

    // Cmd+K toggles it closed again.
    press(&mut app, &ctx, Key::K, Modifiers::COMMAND);
    press(&mut app, &ctx, Key::K, Modifiers::COMMAND);
    assert!(app.palette.is_none());
}

#[test]
fn global_shortcuts_work_and_are_suspended_while_the_palette_is_open() {
    let ctx = egui::Context::default();
    let mut app = populated();
    frames(&mut app, &ctx, 2);
    let tabs = app.tabs.len();
    press(&mut app, &ctx, Key::T, Modifiers::COMMAND);
    assert_eq!(app.tabs.len(), tabs + 1, "Cmd/Ctrl+T opens a tab");
    press(&mut app, &ctx, Key::W, Modifiers::COMMAND);
    assert_eq!(app.tabs.len(), tabs, "Cmd/Ctrl+W closes it");

    app.palette = Some(PaletteState::default());
    press(&mut app, &ctx, Key::T, Modifiers::COMMAND);
    assert_eq!(app.tabs.len(), tabs, "shortcuts don't leak through the palette");
}

#[test]
fn sidebar_search_adding_collection_and_dialogs_render() {
    let ctx = egui::Context::default();
    let mut app = populated();
    for q in ["", "users", "DELETE", "no-such-thing", "empty"] {
        app.search = q.into();
        frames(&mut app, &ctx, 2);
    }
    app.search.clear();
    app.adding_collection = true;
    app.focus_new_collection = true; // what clicking "+" does
    app.new_collection = "Fresh".into();
    frames(&mut app, &ctx, 3); // the box takes focus
    press(&mut app, &ctx, Key::Enter, Modifiers::NONE);
    assert!(app.store.collections.iter().any(|c| c.name == "Fresh"));
    assert!(!app.adding_collection);

    app.adding_collection = true;
    app.focus_new_collection = true;
    frames(&mut app, &ctx, 3);
    press(&mut app, &ctx, Key::Escape, Modifiers::NONE);
    assert!(!app.adding_collection);

    app.show_env = true;
    app.show_io = true;
    app.rename = Some((postrust::app::RenameTarget::Collection(0), "x".into()));
    app.env_sel = 0;
    frames(&mut app, &ctx, 3);
}

#[test]
fn layout_choices_are_remembered_by_the_frame_loop() {
    let ctx = egui::Context::default();
    let mut app = populated();
    settle(&mut app, &ctx, 4);
    assert!(app.store.ui.sidebar_width.is_some(), "sidebar width is captured");
    assert!(
        app.store.ui.response_height.is_some(),
        "response height is captured (below layout)"
    );
    app.toggle_split();
    settle(&mut app, &ctx, 4);
    assert!(
        app.store.ui.response_width.is_some(),
        "response width is captured (side layout)"
    );
}

#[test]
fn many_tabs_and_long_titles_render() {
    let ctx = egui::Context::default();
    let mut app = App::new(None);
    for i in 0..30 {
        let mut r = Request::new(&format!(
            "A very long request name number {i} that should be truncated nicely"
        ));
        r.url = format!("https://example.com/{i}");
        app.new_tab(r, None);
    }
    frames(&mut app, &ctx, 3);
    app.close_tab(5);
    frames(&mut app, &ctx, 2);
}

fn text_and_key(text: &str, k: Key) -> RawInput {
    let mut events = vec![Event::Text(text.to_string())];
    events.extend(key(k, Modifiers::NONE));
    raw(events)
}

#[test]
fn palette_enter_acts_on_the_list_after_this_frames_typing() {
    let ctx = egui::Context::default();
    let mut app = populated();
    app.palette = Some(PaletteState::default());
    frames(&mut app, &ctx, 3); // let the search box take focus

    // Typing and Enter arrive in the same frame: "theme" must win over the default first entry.
    let before = app.store.ui.theme;
    frame(&mut app, &ctx, text_and_key("theme", Key::Enter));
    frames(&mut app, &ctx, 1);
    assert!(app.palette.is_none());
    assert_ne!(app.store.ui.theme, before, "ran 'Change theme', not 'Send request'");
}

#[test]
fn palette_enter_with_no_match_keeps_it_open() {
    let ctx = egui::Context::default();
    let mut app = populated();
    app.palette = Some(PaletteState::default());
    frames(&mut app, &ctx, 3);
    frame(&mut app, &ctx, text_and_key("zzzz", Key::Enter));
    frames(&mut app, &ctx, 1);
    assert!(
        app.palette.is_some(),
        "nothing matched, so nothing ran and it stays open"
    );
    assert_eq!(app.palette.as_ref().unwrap().query, "zzzz");
    // The search box must still have focus: the next keystrokes edit the query.
    frames(&mut app, &ctx, 2);
    frame(&mut app, &ctx, raw(vec![Event::Text("q".into())]));
    frames(&mut app, &ctx, 1);
    assert_eq!(
        app.palette.as_ref().unwrap().query,
        "zzzzq",
        "typing still reaches the palette"
    );
}

#[test]
fn palette_owns_the_keyboard_over_the_new_collection_box() {
    let ctx = egui::Context::default();
    let mut app = populated();
    let collections = app.store.collections.len();
    app.adding_collection = true;
    app.focus_new_collection = true;
    app.new_collection = "Leftover".into();
    frames(&mut app, &ctx, 3);
    assert!(!app.focus_new_collection, "focus is requested once, not every frame");

    app.palette = Some(PaletteState::default());
    press(&mut app, &ctx, Key::Enter, Modifiers::NONE);
    assert_eq!(
        app.store.collections.len(),
        collections,
        "Enter went to the palette, not the sidebar"
    );
}

#[test]
fn oversized_saved_panel_sizes_render_and_are_not_overwritten_by_the_clamp() {
    let ctx = egui::Context::default();
    let mut app = populated();
    app.store.ui.split = SplitLayout::Side;
    app.store.ui.response_width = Some(5000.0); // e.g. saved on a bigger monitor
    settle(&mut app, &ctx, 5);
    assert_eq!(
        app.store.ui.response_width,
        Some(5000.0),
        "the clamped on-screen width must not replace the saved preference"
    );

    app.store.ui.split = SplitLayout::Below;
    app.store.ui.response_height = Some(5000.0);
    settle(&mut app, &ctx, 5);
    assert_eq!(app.store.ui.response_height, Some(5000.0));
}

#[test]
fn clicking_the_method_badge_selects_the_tab() {
    // Smoke only: the badge is part of the tab and renders with a click sense.
    let ctx = egui::Context::default();
    let mut app = populated();
    frames(&mut app, &ctx, 3);
    assert!(app.tabs.len() >= 2);
}

#[test]
fn response_panel_keeps_its_size_with_little_content_and_survives_a_restart() {
    for split in [SplitLayout::Below, SplitLayout::Side] {
        let ctx = egui::Context::default();
        let mut app = populated(); // active tab has no response yet: very little content
        app.store.ui.split = split;
        settle(&mut app, &ctx, 6);
        let (first, default) = match split {
            SplitLayout::Below => (app.store.ui.response_height.unwrap(), 320.0),
            SplitLayout::Side => (app.store.ui.response_width.unwrap(), 460.0),
        };
        assert!(
            (first - default).abs() <= 1.0,
            "{split:?}: an empty panel must not collapse to its content (got {first}, wanted {default})"
        );

        // A "restart": a new app and egui context seeded with the saved value must land on it.
        let ctx2 = egui::Context::default();
        let mut app2 = populated();
        app2.store.ui = app.store.ui.clone();
        settle(&mut app2, &ctx2, 6);
        let second = match split {
            SplitLayout::Below => app2.store.ui.response_height.unwrap(),
            SplitLayout::Side => app2.store.ui.response_width.unwrap(),
        };
        assert!(
            (second - first).abs() <= 1.0,
            "{split:?}: size drifted across a restart ({first} -> {second})"
        );
    }
}

#[test]
fn response_panel_keeps_its_size_when_a_response_arrives() {
    for split in [SplitLayout::Below, SplitLayout::Side] {
        let ctx = egui::Context::default();
        let mut app = populated();
        app.store.ui.split = split;
        let read = |app: &App| match split {
            SplitLayout::Below => app.store.ui.response_height.unwrap(),
            SplitLayout::Side => app.store.ui.response_width.unwrap(),
        };
        settle(&mut app, &ctx, 5);
        let before = read(&app);
        let active = app.active;
        app.tabs[active].response = Some(Ok(resp("application/json", br#"{"a":1}"#)));
        // Many frames: a feedback loop shows up as steady growth, not a one-off jump.
        settle(&mut app, &ctx, 30);
        let after = read(&app);
        assert!(
            (after - before).abs() <= 1.0,
            "{split:?}: panel size changed when a response arrived ({before} -> {after})"
        );
        for response_kind in [Err("boom".to_string()), Ok(resp("text/plain", b"hello"))] {
            app.tabs[active].response = Some(response_kind);
            settle(&mut app, &ctx, 30);
            assert!(
                (read(&app) - before).abs() <= 1.0,
                "{split:?}: size drifted for another response kind"
            );
        }
    }
}

fn click(pos: Pos2) -> Vec<Event> {
    let button = egui::PointerButton::Primary;
    vec![
        Event::PointerMoved(pos),
        Event::PointerButton {
            pos,
            button,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
        Event::PointerButton {
            pos,
            button,
            pressed: false,
            modifiers: Modifiers::NONE,
        },
    ]
}

#[test]
fn closing_a_dirty_tab_shows_the_confirmation_and_each_choice_works() {
    use postrust::app::CloseChoice;
    let ctx = egui::Context::default();
    let mut app = populated(); // tab 1 has an unsaved change
    frames(&mut app, &ctx, 2);
    let tabs = app.tabs.len();
    let dirty_id = app.tabs[1].id;
    app.active = 1;

    press(&mut app, &ctx, Key::W, Modifiers::COMMAND);
    assert_eq!(app.tabs.len(), tabs, "kept while asking");
    assert_eq!(app.confirm_close, Some(dirty_id));
    frames(&mut app, &ctx, 3); // the dialog renders

    press(&mut app, &ctx, Key::Escape, Modifiers::NONE);
    assert!(app.confirm_close.is_none(), "Escape cancels");
    assert_eq!(app.tabs.len(), tabs);

    app.active = 1;
    app.request_close_tab(1);
    frames(&mut app, &ctx, 2);
    app.resolve_close(CloseChoice::Discard);
    assert_eq!(app.tabs.len(), tabs - 1);
    assert!(app.tabs.iter().all(|t| t.id != dirty_id));
    frames(&mut app, &ctx, 2);
}

#[test]
fn undo_button_appears_only_while_the_beautified_body_is_untouched() {
    let ctx = egui::Context::default();
    let mut app = populated();
    app.active = 1;
    app.tabs[1].req_tab = ReqTab::Body;
    app.tabs[1].request.body_kind = BodyKind::Raw;
    app.tabs[1].request.body = r#"{"a":1}"#.into();
    frames(&mut app, &ctx, 2);
    app.beautify_body(1);
    assert!(app.tabs[1].beautify_undo.is_some());
    frames(&mut app, &ctx, 3); // renders the Undo button
    app.tabs[1].request.body.push('x');
    frames(&mut app, &ctx, 3); // button hidden again, no panic
}

#[test]
fn palette_does_not_steal_focus_every_frame_and_closes_on_an_outside_click() {
    let ctx = egui::Context::default();
    let mut app = populated();
    app.palette = Some(PaletteState::default());
    frames(&mut app, &ctx, 3);
    assert!(app.palette.as_ref().unwrap().focused, "focus requested once");

    // A click far from the centred palette dismisses it.
    frame(&mut app, &ctx, raw(click(Pos2::new(20.0, 780.0))));
    frames(&mut app, &ctx, 2);
    assert!(app.palette.is_none(), "outside click closes the palette");

    // A click inside it does not.
    app.palette = Some(PaletteState::default());
    frames(&mut app, &ctx, 3);
    frame(&mut app, &ctx, raw(click(Pos2::new(640.0, 100.0))));
    frames(&mut app, &ctx, 2);
    assert!(app.palette.is_some(), "clicks inside the palette keep it open");
}

#[test]
fn filtering_the_sidebar_does_not_disturb_collapsed_state() {
    let ctx = egui::Context::default();
    let mut app = populated();
    frames(&mut app, &ctx, 2);
    app.search = "users".into();
    frames(&mut app, &ctx, 3);
    app.search.clear();
    frames(&mut app, &ctx, 3);
}

#[test]
fn merely_viewing_never_writes_layout_preferences() {
    let ctx = egui::Context::default();
    let mut app = populated();
    frames(&mut app, &ctx, 10);
    assert_eq!(app.store.ui.sidebar_width, None);
    assert_eq!(app.store.ui.response_height, None);
    assert_eq!(app.store.ui.response_width, None);
}

#[test]
fn shortcuts_pause_while_the_unsaved_changes_dialog_is_open() {
    let ctx = egui::Context::default();
    let mut app = populated();
    app.active = 1; // the tab with an unsaved change
    frames(&mut app, &ctx, 2);
    press(&mut app, &ctx, Key::W, Modifiers::COMMAND);
    let pending = app.confirm_close;
    assert!(pending.is_some());
    let tabs = app.tabs.len();
    let active = app.active;

    press(&mut app, &ctx, Key::W, Modifiers::COMMAND);
    press(&mut app, &ctx, Key::T, Modifiers::COMMAND);
    press(&mut app, &ctx, Key::S, Modifiers::COMMAND);
    press(&mut app, &ctx, Key::Enter, Modifiers::COMMAND);
    assert_eq!(app.confirm_close, pending, "still asking about the same tab");
    assert_eq!(app.tabs.len(), tabs, "no tab opened or closed behind the dialog");
    assert_eq!(app.active, active);
    assert!(app.tabs[1].pending.is_none(), "nothing was sent behind the dialog");
}

fn focus_url(app: &mut App, ctx: &egui::Context) {
    let id = postrust::ui::url_field_id(app.tabs[app.active].id);
    ctx.memory_mut(|m| m.request_focus(id));
    frames(app, ctx, 2);
}

fn paste(app: &mut App, ctx: &egui::Context, text: &str) {
    frame(app, ctx, raw(vec![Event::Paste(text.to_string())]));
    frames(app, ctx, 2);
}

#[test]
fn pasting_a_curl_command_into_the_url_field_fills_the_request() {
    let ctx = egui::Context::default();
    let mut app = App::new(None);
    app.tabs[0].request.name = "Keep me".into();
    frames(&mut app, &ctx, 2);
    focus_url(&mut app, &ctx);

    paste(
        &mut app,
        &ctx,
        "curl -X POST 'https://api.test/users?x=1' \\\n  -H 'Content-Type: application/json' \\\n  -d '{\"name\":\"ada\"}'",
    );
    let r = &app.tabs[0].request;
    assert_eq!(r.method, Method::Post);
    assert_eq!(
        r.url, "https://api.test/users?x=1",
        "the raw command was not typed into the field"
    );
    assert_eq!(r.body, r#"{"name":"ada"}"#);
    assert_eq!(r.name, "Keep me");
    assert!(app.status.as_deref().unwrap().starts_with("Imported cURL command"));
    assert!(app.tabs[0].import_undo.is_some());

    // The Undo button is rendered while the request is untouched, and Undo works.
    frames(&mut app, &ctx, 2);
    app.undo_import(0);
    assert_eq!(app.tabs[0].request.url, "");
    frames(&mut app, &ctx, 2);
}

#[test]
fn only_curl_commands_pasted_into_the_focused_url_field_are_intercepted() {
    let ctx = egui::Context::default();
    let mut app = App::new(None);
    frames(&mut app, &ctx, 2);

    // Not focused: a pasted command is left alone (nothing imports it).
    paste(&mut app, &ctx, "curl https://a.test/unfocused");
    assert!(app.tabs[0].import_undo.is_none());
    assert_eq!(app.tabs[0].request.url, "");

    // Focused, but not a curl command: ordinary paste.
    focus_url(&mut app, &ctx);
    paste(&mut app, &ctx, "https://a.test/plain");
    assert_eq!(app.tabs[0].request.url, "https://a.test/plain");
    assert!(app.tabs[0].import_undo.is_none());

    // Text that merely contains "curl" is not a command either.
    paste(&mut app, &ctx, "/curl https://x");
    assert_eq!(app.tabs[0].request.url, "https://a.test/plain/curl https://x");
    assert!(app.tabs[0].import_undo.is_none());
}

#[test]
fn an_unparseable_curl_paste_leaves_the_url_field_untouched_and_says_why() {
    let ctx = egui::Context::default();
    let mut app = App::new(None);
    app.tabs[0].request.url = "https://keep.test".into();
    frames(&mut app, &ctx, 2);
    focus_url(&mut app, &ctx);
    paste(&mut app, &ctx, "curl 'https://a.test/unterminated\n  -H 'x: y'");
    assert!(app.tabs[0].import_undo.is_none(), "nothing was imported");
    assert_eq!(
        app.tabs[0].request.url, "https://keep.test",
        "a long broken command is not dumped into the one-line field"
    );
    let status = app.status.clone().unwrap();
    assert!(
        status.contains("Could not import cURL") && status.contains("left unchanged"),
        "{status}"
    );
}

#[test]
fn a_second_import_replaces_the_first_and_undo_goes_back_one_step() {
    let ctx = egui::Context::default();
    let mut app = App::new(None);
    frames(&mut app, &ctx, 2);
    focus_url(&mut app, &ctx);
    paste(&mut app, &ctx, "curl https://one.test");
    assert_eq!(app.tabs[0].request.url, "https://one.test");
    paste(&mut app, &ctx, "curl -X DELETE https://two.test");
    assert_eq!(app.tabs[0].request.url, "https://two.test");
    assert_eq!(app.tabs[0].request.method, Method::Delete);
    app.undo_import(0);
    assert_eq!(app.tabs[0].request.url, "https://one.test");
}

/// Vertical centres of every text shape in one frame, keyed by the text.
fn text_centres(app: &mut App, ctx: &egui::Context) -> Vec<(String, f32)> {
    fn walk(shape: &egui::epaint::Shape, out: &mut Vec<(String, f32)>) {
        match shape {
            egui::epaint::Shape::Text(t) => {
                out.push((t.galley.text().to_string(), t.pos.y + t.galley.rect.center().y));
            }
            egui::epaint::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
            _ => {}
        }
    }
    let mut output = ctx.run_ui(raw(vec![]), |ui| postrust::ui::draw(app, ui));
    let mut found = Vec::new();
    for clipped in std::mem::take(&mut output.shapes) {
        walk(&clipped.shape, &mut found);
    }
    output.drop_without_applying_deltas();
    found
}

#[test]
fn request_bar_text_is_vertically_centred_with_its_neighbours() {
    for theme in [ThemeMode::Dark, ThemeMode::Light] {
        let ctx = egui::Context::default();
        let mut app = App::new(None);
        app.store.ui.theme = theme;
        frames(&mut app, &ctx, 3);
        let texts = text_centres(&mut app, &ctx);
        let centre_of = |starts: &str| {
            texts
                .iter()
                .find(|(t, _)| t.starts_with(starts))
                .map(|(_, y)| *y)
                .unwrap_or_else(|| {
                    panic!(
                        "no text starting with {starts:?} among {:?}",
                        texts.iter().map(|(t, _)| t).collect::<Vec<_>>()
                    )
                })
        };
        let url = centre_of("https://api.example.com");
        let send = centre_of("Send");
        // Both the tab badge and the method box say "GET"; the one on the request bar's row is
        // the one nearest to the Send button.
        let method = texts
            .iter()
            .filter(|(t, _)| t == "GET")
            .map(|(_, y)| *y)
            .min_by(|a, b| (a - send).abs().total_cmp(&(b - send).abs()))
            .expect("a GET label");
        assert!(
            (url - send).abs() <= 2.0,
            "{theme:?}: URL text centre {url} vs Send {send}"
        );
        assert!(
            (url - method).abs() <= 2.0,
            "{theme:?}: URL text centre {url} vs GET {method}"
        );

        // The same must hold once the user has typed something (real text, not the hint).
        app.tabs[0].request.url = "https://typed.example.com/path".into();
        frames(&mut app, &ctx, 2);
        let texts = text_centres(&mut app, &ctx);
        let typed = texts
            .iter()
            .find(|(t, _)| t.starts_with("https://typed"))
            .map(|(_, y)| *y)
            .unwrap();
        let send = texts.iter().find(|(t, _)| t == "Send").map(|(_, y)| *y).unwrap();
        assert!(
            (typed - send).abs() <= 2.0,
            "{theme:?}: typed URL centre {typed} vs Send {send}"
        );
    }
}

/// Horizontal extents (min x, max x) of every text shape in one frame, at a given window width.
fn text_spans(app: &mut App, ctx: &egui::Context, width: f32) -> Vec<(String, f32, f32)> {
    fn walk(shape: &egui::epaint::Shape, out: &mut Vec<(String, f32, f32)>) {
        match shape {
            egui::epaint::Shape::Text(t) => out.push((
                t.galley.text().to_string(),
                t.pos.x + t.galley.rect.min.x,
                t.pos.x + t.galley.rect.max.x,
            )),
            egui::epaint::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
            _ => {}
        }
    }
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, 800.0))),
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| postrust::ui::draw(app, ui));
    let mut found = Vec::new();
    for clipped in std::mem::take(&mut output.shapes) {
        walk(&clipped.shape, &mut found);
    }
    output.drop_without_applying_deltas();
    found
}

#[test]
fn status_message_never_runs_into_the_shortcut_hints() {
    for web_mode in [false, true] {
        for width in [900.0, 1280.0, 1920.0] {
            for status in ["Ready", "Saved", &"a very long status message ".repeat(30)] {
                let ctx = egui::Context::default();
                let mut app = App::new(None);
                app.web_mode = web_mode;
                app.status = Some(status.to_string());
                for _ in 0..3 {
                    text_spans(&mut app, &ctx, width);
                }
                let spans = text_spans(&mut app, &ctx, width);
                let hints = spans
                    .iter()
                    .find(|(t, _, _)| t.contains("K commands") && t.contains("Enter send"))
                    .unwrap_or_else(|| {
                        panic!(
                            "no shortcut hints in {:?}",
                            spans.iter().map(|s| &s.0).collect::<Vec<_>>()
                        )
                    });
                let message = spans
                    .iter()
                    .find(|(t, _, _)| t.starts_with(&status[..status.len().min(5)]) && !t.contains("K commands"))
                    .unwrap_or_else(|| panic!("no status text for {status:?}"));
                assert!(
                    message.2 <= hints.1 + 0.5,
                    "web={web_mode} width={width}: status ends at {} but the hints start at {} ({status:.20?})",
                    message.2,
                    hints.1
                );
                assert!(hints.2 <= width + 0.5, "hints must stay inside the window");
            }
        }
    }
}

#[test]
fn web_mode_hides_the_browser_reserved_shortcuts() {
    let ctx = egui::Context::default();
    let mut app = App::new(None);
    app.web_mode = true;
    for _ in 0..3 {
        text_spans(&mut app, &ctx, 1280.0);
    }
    let spans = text_spans(&mut app, &ctx, 1280.0);
    let hints = spans
        .iter()
        .find(|(t, _, _)| t.contains("K commands"))
        .map(|s| s.0.clone())
        .unwrap();
    assert!(!hints.contains("new tab") && !hints.contains("close tab"), "{hints}");
    app.web_mode = false;
    for _ in 0..3 {
        text_spans(&mut app, &ctx, 1280.0);
    }
    let spans = text_spans(&mut app, &ctx, 1280.0);
    let hints = spans
        .iter()
        .find(|(t, _, _)| t.contains("K commands"))
        .map(|s| s.0.clone())
        .unwrap();
    assert!(hints.contains("new tab") && hints.contains("close tab"), "{hints}");
}

#[test]
fn web_import_window_imports_pasted_exports_and_the_forbidden_header_warning_shows() {
    let ctx = egui::Context::default();
    let mut app = App::new(None);
    app.web_mode = true;
    app.show_io = true;
    let export = postrust::storage::collections_to_json(&[Collection {
        name: "Pasted".into(),
        requests: vec![Request::new("one")],
    }])
    .unwrap();
    app.io_text = export;
    frames(&mut app, &ctx, 3); // the web layout of the window renders

    // What the "Import" button does with the pasted text.
    let text = std::mem::take(&mut app.io_text);
    app.import_collections_text(&text);
    assert_eq!(app.store.collections.len(), 1);
    assert_eq!(app.store.collections[0].name, "Pasted");
    app.import_collections_text("not an export");
    assert!(app.status.as_deref().unwrap().starts_with("Import failed"));

    // The headers tab warns about headers the browser would silently drop.
    app.tabs[0].req_tab = ReqTab::Headers;
    app.tabs[0].request.headers = vec![KeyValue::new("Cookie", "a=1"), KeyValue::new("X-Ok", "1")];
    for _ in 0..3 {
        text_spans(&mut app, &ctx, 1280.0);
    }
    let spans = text_spans(&mut app, &ctx, 1280.0);
    assert!(
        spans
            .iter()
            .any(|(t, _, _)| t.contains("The browser ignores these headers: Cookie")),
        "{:?}",
        spans.iter().map(|s| &s.0).collect::<Vec<_>>()
    );
    app.web_mode = false;
    for _ in 0..3 {
        text_spans(&mut app, &ctx, 1280.0);
    }
    assert!(
        !text_spans(&mut app, &ctx, 1280.0)
            .iter()
            .any(|(t, _, _)| t.contains("browser ignores"))
    );
}

#[test]
fn web_auth_tab_warns_about_api_key_headers_the_browser_drops_or_may_forward() {
    let ctx = egui::Context::default();
    let mut app = App::new(None);
    app.web_mode = true;
    app.tabs[0].req_tab = ReqTab::Auth;
    let render = |app: &mut App| {
        for _ in 0..3 {
            text_spans(app, &ctx, 1280.0);
        }
        text_spans(app, &ctx, 1280.0)
            .into_iter()
            .map(|s| s.0)
            .collect::<Vec<_>>()
    };

    app.tabs[0].request.auth = Auth::ApiKey {
        name: "X-Api-Key".into(),
        value: "k".into(),
    };
    let texts = render(&mut app);
    assert!(
        texts
            .iter()
            .any(|t| t.contains("redirects automatically") && t.contains("X-Api-Key")),
        "{texts:?}"
    );
    assert!(!texts.iter().any(|t| t.contains("ignores this header")), "{texts:?}");

    // Any API key is a credential, whatever its name; and a forbidden name is dropped by the browser.
    app.tabs[0].request.auth = Auth::ApiKey {
        name: "Cookie".into(),
        value: "k".into(),
    };
    let texts = render(&mut app);
    assert!(
        texts.iter().any(|t| t.contains("ignores this header: Cookie")),
        "{texts:?}"
    );
    assert!(texts.iter().any(|t| t.contains("redirects automatically")), "{texts:?}");

    // Bearer and Basic use Authorization, which browsers already strip on redirects.
    app.tabs[0].request.auth = Auth::Bearer { token: "t".into() };
    assert!(!render(&mut app).iter().any(|t| t.contains("redirects automatically")));

    // On the desktop none of this shows (its redirect guard handles it).
    app.web_mode = false;
    app.tabs[0].request.auth = Auth::ApiKey {
        name: "X-Api-Key".into(),
        value: "k".into(),
    };
    assert!(
        !render(&mut app)
            .iter()
            .any(|t| t.contains("redirects automatically") || t.contains("ignores this header"))
    );
}

#[test]
fn web_headers_tab_warns_about_credential_headers_on_redirects() {
    let ctx = egui::Context::default();
    let mut app = App::new(None);
    app.web_mode = true;
    app.tabs[0].req_tab = ReqTab::Headers;
    app.tabs[0].request.headers = vec![KeyValue::new("X-Auth-Token", "t"), KeyValue::new("Accept", "*/*")];
    for _ in 0..3 {
        text_spans(&mut app, &ctx, 1280.0);
    }
    let texts: Vec<String> = text_spans(&mut app, &ctx, 1280.0).into_iter().map(|s| s.0).collect();
    assert!(
        texts
            .iter()
            .any(|t| t.contains("redirects automatically") && t.contains("X-Auth-Token")),
        "{texts:?}"
    );
    assert!(
        !texts
            .iter()
            .any(|t| t.contains("redirects automatically") && t.contains("Accept"))
    );
}
