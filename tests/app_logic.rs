use postrust::app::App;
use postrust::model::*;

fn req(name: &str) -> Request {
    Request::new(name)
}

#[test]
fn save_creates_default_collection_then_overwrites() {
    let mut app = App::new(None);
    app.tabs[0].request = req("one");
    app.save_tab(0);
    assert_eq!(app.store.collections.len(), 1);
    assert_eq!(app.tabs[0].source, Some((0, 0)));

    app.tabs[0].request.url = "http://x".into();
    app.save_tab(0);
    assert_eq!(app.store.collections[0].requests.len(), 1);
    assert_eq!(app.store.collections[0].requests[0].url, "http://x");
}

#[test]
fn opening_same_saved_request_focuses_existing_tab() {
    let mut app = App::new(None);
    app.store.collections.push(Collection {
        name: "c".into(),
        requests: vec![req("a"), req("b")],
    });
    app.open_saved(0, 1);
    app.open_saved(0, 0);
    let n = app.tabs.len();
    app.open_saved(0, 1);
    assert_eq!(app.tabs.len(), n);
    assert_eq!(app.tabs[app.active].source, Some((0, 1)));
}

#[test]
fn closing_last_tab_leaves_a_fresh_one() {
    let mut app = App::new(None);
    app.close_tab(0);
    assert_eq!(app.tabs.len(), 1);
}

#[test]
fn closing_earlier_tab_keeps_active_tab_selected() {
    let mut app = App::new(None);
    app.new_tab(req("b"), None);
    app.new_tab(req("c"), None);
    app.active = 2;
    app.close_tab(0);
    assert_eq!(app.tabs[app.active].request.name, "c");
}

#[test]
fn deleting_request_detaches_tabs_of_that_collection() {
    let mut app = App::new(None);
    app.store.collections.push(Collection {
        name: "c".into(),
        requests: vec![req("a"), req("b")],
    });
    app.open_saved(0, 1);
    app.delete_request(0, 0);
    // "b" moved from slot 1 to slot 0; the tab must follow it, so Save overwrites "b".
    assert_eq!(app.tabs[app.active].source, Some((0, 0)));
    app.tabs[app.active].request.url = "http://changed".into();
    let idx = app.active;
    app.save_tab(idx);
    assert_eq!(app.store.collections[0].requests.len(), 1);
    assert_eq!(app.store.collections[0].requests[0].url, "http://changed");
}

#[test]
fn deleting_a_request_detaches_only_its_own_tab() {
    let mut app = App::new(None);
    app.store.collections.push(Collection {
        name: "c".into(),
        requests: vec![req("a"), req("b"), req("c")],
    });
    app.open_saved(0, 0);
    app.open_saved(0, 1);
    app.open_saved(0, 2);
    app.delete_request(0, 1);
    let sources: Vec<_> = app.tabs.iter().filter_map(|t| t.source).collect();
    assert_eq!(sources, vec![(0, 0), (0, 1)]);
    assert!(app.tabs.iter().any(|t| t.request.name == "b" && t.source.is_none()));
}

#[test]
fn delete_clears_pending_rename() {
    use postrust::app::RenameTarget;
    let mut app = App::new(None);
    app.store.collections.push(Collection {
        name: "c".into(),
        requests: vec![req("a"), req("b")],
    });
    app.rename = Some((RenameTarget::Request(0, 1), "x".into()));
    app.delete_request(0, 0);
    assert!(app.rename.is_none());
}

#[test]
fn export_refuses_to_overwrite_foreign_file() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("notes.json");
    std::fs::write(&p, "{\"precious\": true}").unwrap();
    let mut app = App::new(None);
    app.io_path = p.display().to_string();
    app.export_collections();
    assert!(app.status.as_deref().unwrap().starts_with("Export refused"));
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "{\"precious\": true}");
    // A previous export may be replaced.
    let q = dir.path().join("again.json");
    app.io_path = q.display().to_string();
    app.export_collections();
    app.export_collections();
    assert!(app.status.as_deref().unwrap().starts_with("Exported"));
}

#[test]
fn deleting_collection_reindexes_later_collections() {
    let mut app = App::new(None);
    app.store.collections.push(Collection {
        name: "a".into(),
        requests: vec![req("x")],
    });
    app.store.collections.push(Collection {
        name: "b".into(),
        requests: vec![req("y")],
    });
    app.open_saved(1, 0);
    app.open_saved(0, 0);
    app.delete_collection(0);
    // Collection "b" moved from index 1 to 0 and its tab follows; the deleted one detaches.
    let sources: Vec<_> = app.tabs.iter().filter_map(|t| t.source).collect();
    assert_eq!(sources, vec![(0, 0)]);
}

#[test]
fn deleting_environment_fixes_active_selection() {
    let mut app = App::new(None);
    for n in ["a", "b", "c"] {
        app.store.environments.push(Environment {
            name: n.into(),
            vars: vec![],
        });
    }
    app.store.active_env = Some(2);
    app.delete_environment(0);
    assert_eq!(app.store.active_env, Some(1));
    app.delete_environment(1);
    assert_eq!(app.store.active_env, None);
}

#[test]
fn missing_vars_use_active_environment() {
    let mut app = App::new(None);
    app.store.environments.push(Environment {
        name: "e".into(),
        vars: vec![KeyValue::new("host", "h")],
    });
    let mut r = req("x");
    r.url = "{{host}}/{{id}}".into();
    assert_eq!(app.missing_vars(&r), vec!["host", "id"]);
    app.store.active_env = Some(0);
    assert_eq!(app.missing_vars(&r), vec!["id"]);
    r.url = "{{host}}".into();
    assert!(app.missing_vars(&r).is_empty());
    app.store.active_env = None;
    assert_eq!(app.missing_vars(&r), vec!["host"]);
}

#[test]
fn history_dedupes_and_caps() {
    let mut store = Store::default();
    for i in 0..(HISTORY_LIMIT + 10) {
        let mut r = req("x");
        r.url = format!("http://h/{i}");
        store.push_history(&r);
    }
    assert_eq!(store.history.len(), HISTORY_LIMIT);
    let first = store.history[0].clone();
    store.push_history(&first);
    assert_eq!(store.history.len(), HISTORY_LIMIT);
}

#[test]
fn import_export_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(None);
    app.store.collections.push(Collection {
        name: "c".into(),
        requests: vec![req("a")],
    });
    app.io_path = dir.path().join("out.json").display().to_string();
    app.export_collections();
    app.import_collections();
    assert_eq!(app.store.collections.len(), 2);
    app.io_path = dir.path().join("missing.json").display().to_string();
    app.import_collections();
    assert!(app.status.as_deref().unwrap().starts_with("Import failed"));
}

#[test]
fn send_with_empty_url_reports_error_without_pending() {
    let mut app = App::new(None);
    let ctx = egui::Context::default();
    app.send(0, &ctx);
    assert!(app.tabs[0].pending.is_none());
    assert!(matches!(app.tabs[0].response, Some(Err(_))));
    assert!(app.store.history.is_empty());
}

#[test]
fn rename_updates_store_and_open_tabs() {
    use postrust::app::RenameTarget;
    let mut app = App::new(None);
    app.store.collections.push(Collection {
        name: "c".into(),
        requests: vec![req("a")],
    });
    app.open_saved(0, 0);
    app.rename(&RenameTarget::Request(0, 0), "  renamed ");
    assert_eq!(app.store.collections[0].requests[0].name, "renamed");
    assert_eq!(app.tabs[app.active].request.name, "renamed");
    app.rename(&RenameTarget::Collection(0), "   ");
    assert_eq!(app.store.collections[0].name, "c");
    app.rename(&RenameTarget::Collection(0), "new");
    assert_eq!(app.store.collections[0].name, "new");
}

#[test]
fn add_collection_ignores_blank_and_clear_history_empties() {
    let mut app = App::new(None);
    app.add_collection("   ");
    assert!(app.store.collections.is_empty());
    app.add_collection(" x ");
    assert_eq!(app.store.collections[0].name, "x");
    app.store.push_history(&req("h"));
    app.clear_history();
    assert!(app.store.history.is_empty());
}

#[test]
fn disabled_rows_do_not_trigger_missing_var_warning() {
    let app = App::new(None);
    let mut r = req("x");
    let mut off = KeyValue::new("X", "{{ghost}}");
    off.enabled = false;
    r.headers = vec![off];
    assert!(app.missing_vars(&r).is_empty());
}

#[test]
fn history_never_stores_auth_credentials() {
    let mut app = App::new(None);
    app.tabs[0].request.url = "http://127.0.0.1:1".into();
    app.tabs[0].request.auth = Auth::Bearer { token: "secret".into() };
    app.send(0, &egui::Context::default());
    assert_eq!(app.store.history.len(), 1);
    assert_eq!(app.store.history[0].auth, Auth::None);
    // The live request keeps its credentials.
    assert!(matches!(app.tabs[0].request.auth, Auth::Bearer { .. }));
}

#[test]
fn import_rejects_arbitrary_json_arrays() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("x.json");
    std::fs::write(&p, r#"[{"id":1},{"id":2}]"#).unwrap();
    let mut app = App::new(None);
    app.io_path = p.display().to_string();
    app.import_collections();
    assert!(app.store.collections.is_empty());
    assert!(app.status.as_deref().unwrap().starts_with("Import failed"));
    app.export_collections();
    assert!(app.status.as_deref().unwrap().starts_with("Export refused"));
    assert_eq!(std::fs::read_to_string(&p).unwrap(), r#"[{"id":1},{"id":2}]"#);
}

#[test]
fn timeout_and_active_env_setters_are_clamped() {
    let mut app = App::new(None);
    app.set_timeout(0);
    assert_eq!(app.store.timeout_secs, Some(1));
    app.set_timeout(99_999);
    assert_eq!(app.store.timeout_secs, Some(600));
    app.set_active_env(Some(5));
    assert_eq!(app.store.active_env, None);
}

#[test]
fn out_of_range_deletes_change_nothing() {
    let mut app = App::new(None);
    app.store.collections.push(Collection {
        name: "c".into(),
        requests: vec![req("a"), req("b")],
    });
    app.open_saved(0, 1);
    app.delete_request(0, 9);
    app.delete_request(7, 0);
    app.delete_collection(5);
    assert_eq!(app.tabs[app.active].source, Some((0, 1)));
    assert_eq!(app.store.collections[0].requests.len(), 2);
}

#[test]
fn deleting_earlier_environment_keeps_selection_on_same_env() {
    let mut app = App::new(None);
    for n in ["a", "b", "c", "d"] {
        app.store.environments.push(Environment {
            name: n.into(),
            vars: vec![],
        });
    }
    app.env_sel = 2;
    app.delete_environment(0);
    assert_eq!(app.store.environments[app.env_sel].name, "c");
}

#[test]
fn running_without_storage_warns() {
    let app = App::new(None);
    assert!(app.status.as_deref().unwrap().contains("will not be saved"));
}

#[test]
fn send_is_refused_for_undefined_variables_in_url_or_auth() {
    let mut app = App::new(None);
    let ctx = egui::Context::default();
    app.tabs[0].request.url = "{{base}}/users".into();
    app.send(0, &ctx);
    assert!(app.tabs[0].pending.is_none());
    match &app.tabs[0].response {
        Some(Err(msg)) => assert!(msg.contains("base"), "{msg}"),
        _ => panic!("expected an error"),
    }
    assert!(app.store.history.is_empty());

    app.tabs[0].request.url = "http://127.0.0.1:1".into();
    app.tabs[0].request.auth = Auth::Bearer {
        token: "{{token}}".into(),
    };
    app.send(0, &ctx);
    assert!(app.tabs[0].pending.is_none());
    assert!(matches!(&app.tabs[0].response, Some(Err(m)) if m.contains("token")));

    // Body-only variables don't block sending (they show a warning instead).
    app.tabs[0].request.auth = Auth::None;
    app.tabs[0].request.body_kind = BodyKind::Raw;
    app.tabs[0].request.body = "{{mustache}}".into();
    app.send(0, &ctx);
    assert!(app.tabs[0].pending.is_some());
}

#[test]
fn dirty_dot_tracks_unsaved_changes() {
    let mut app = App::new(None);
    assert!(!app.is_tab_dirty(0), "a blank tab is not dirty");
    app.tabs[0].request.url = "http://x".into();
    assert!(app.is_tab_dirty(0), "an unsaved tab with content is dirty");
    app.save_tab(0);
    assert!(!app.is_tab_dirty(0));
    app.tabs[0].request.body = "changed".into();
    assert!(app.is_tab_dirty(0));
    app.tabs[0].request.body.clear();
    assert!(!app.is_tab_dirty(0), "reverting the edit clears the dot");
    assert!(!app.is_tab_dirty(99));
}

#[test]
fn theme_and_layout_prefs_round_trip_through_storage() {
    use postrust::model::{SplitLayout, ThemeMode};
    use postrust::storage::Storage;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.json");
    {
        let mut app = App::new(Some(Storage::new(path.clone())));
        app.set_theme(ThemeMode::Dark);
        app.toggle_split();
        app.remember_sidebar_width(301.5);
        app.remember_response_size(SplitLayout::Side, 420.0);
        app.remember_response_size(SplitLayout::Below, 280.0);
        app.mark_dirty();
        // Saving happens in the frame loop; do it explicitly through the storage API.
        Storage::new(path.clone()).save(&app.store).unwrap();
    }
    let loaded = Storage::new(path).load();
    assert_eq!(loaded.ui.theme, ThemeMode::Dark);
    assert_eq!(loaded.ui.split, SplitLayout::Side);
    assert_eq!(loaded.ui.sidebar_width, Some(301.5));
    assert_eq!(loaded.ui.response_width, Some(420.0));
    assert_eq!(loaded.ui.response_height, Some(280.0));
}

#[test]
fn old_store_files_without_ui_prefs_still_load() {
    use postrust::model::{SplitLayout, ThemeMode};
    let store: Store = serde_json::from_str(r#"{"collections":[],"timeout_secs":5}"#).unwrap();
    assert_eq!(store.ui.theme, ThemeMode::System);
    assert_eq!(store.ui.split, SplitLayout::Below);
    assert_eq!(store.ui.sidebar_width, None);
}

#[test]
fn panel_sizes_ignore_noise_and_garbage() {
    let mut app = App::new(None);
    app.remember_sidebar_width(250.0);
    assert_eq!(app.store.ui.sidebar_width, Some(250.0));
    app.remember_sidebar_width(250.4);
    assert_eq!(app.store.ui.sidebar_width, Some(250.0), "sub-pixel change ignored");
    app.remember_sidebar_width(f32::NAN);
    app.remember_sidebar_width(-5.0);
    app.remember_sidebar_width(0.0);
    assert_eq!(app.store.ui.sidebar_width, Some(250.0));
    app.remember_sidebar_width(300.0);
    assert_eq!(app.store.ui.sidebar_width, Some(300.0));
}

#[test]
fn commands_drive_the_app() {
    use postrust::commands::Command;
    use postrust::model::{SplitLayout, ThemeMode};
    let ctx = egui::Context::default();
    let mut app = App::new(None);

    app.run_command(Command::NewTab, &ctx);
    assert_eq!(app.tabs.len(), 2);
    app.run_command(Command::CloseTab, &ctx);
    assert_eq!(app.tabs.len(), 1);

    let mut seen = Vec::new();
    for _ in 0..3 {
        app.run_command(Command::CycleTheme, &ctx);
        seen.push(app.store.ui.theme);
    }
    assert_eq!(seen, vec![ThemeMode::Light, ThemeMode::Dark, ThemeMode::System]);

    app.run_command(Command::ToggleSplit, &ctx);
    assert_eq!(app.store.ui.split, SplitLayout::Side);

    app.run_command(Command::ManageEnvironments, &ctx);
    app.run_command(Command::ImportExport, &ctx);
    assert!(app.show_env && app.show_io);

    app.tabs[0].request.url = "http://x".into();
    app.run_command(Command::Save, &ctx);
    assert_eq!(app.store.collections[0].requests.len(), 1);
}

#[test]
fn beautify_command_reports_what_it_did() {
    use postrust::commands::Command;
    let ctx = egui::Context::default();
    let mut app = App::new(None);

    app.run_command(Command::BeautifyBody, &ctx);
    assert!(app.status.as_deref().unwrap().contains("Raw"));

    app.tabs[0].request.body_kind = BodyKind::Raw;
    app.run_command(Command::BeautifyBody, &ctx);
    assert!(app.status.as_deref().unwrap().contains("empty"));

    app.tabs[0].request.body = r#"{"id":{{id}},"a":1}"#.into();
    app.run_command(Command::BeautifyBody, &ctx);
    assert!(app.status.as_deref().unwrap().starts_with("Body beautified"));
    assert!(app.tabs[0].request.body.contains("\n  \"id\": {{id}}"));

    app.tabs[0].request.body = "{oops".into();
    app.run_command(Command::BeautifyBody, &ctx);
    assert!(app.status.as_deref().unwrap().starts_with("Cannot beautify"));
    assert_eq!(app.tabs[0].request.body, "{oops", "invalid JSON is left untouched");
}

#[test]
fn copy_curl_resolves_variables_and_reports_errors() {
    use postrust::commands::Command;
    let ctx = egui::Context::default();
    let mut app = App::new(None);
    app.run_command(Command::CopyCurl, &ctx);
    assert!(
        app.status.as_deref().unwrap().starts_with("Cannot build cURL"),
        "empty URL"
    );

    app.store.environments.push(Environment {
        name: "e".into(),
        vars: vec![KeyValue::new("host", "h.test")],
    });
    app.store.active_env = Some(0);
    app.tabs[0].request.url = "http://{{host}}/x".into();
    app.run_command(Command::CopyCurl, &ctx);
    assert_eq!(app.status.as_deref(), Some("Copied cURL command to the clipboard"));
}

#[test]
fn sidebar_search_matches_name_url_and_method() {
    let mut r = req("Create user");
    r.url = "https://api.test/Users".into();
    r.method = Method::Post;
    assert!(r.matches(""));
    assert!(r.matches("  "));
    assert!(r.matches("create"));
    assert!(r.matches("USERS"));
    assert!(r.matches("post"));
    assert!(!r.matches("delete"));
}

#[test]
fn beautify_can_be_undone_until_the_body_is_edited() {
    use postrust::commands::Command;
    let ctx = egui::Context::default();
    let mut app = App::new(None);
    let original = r#"{"b":1,"a":2}"#;
    app.tabs[0].request.body_kind = BodyKind::Raw;
    app.tabs[0].request.body = original.into();
    app.run_command(Command::BeautifyBody, &ctx);
    assert_ne!(app.tabs[0].request.body, original);

    app.undo_beautify(0);
    assert_eq!(app.tabs[0].request.body, original);
    assert_eq!(app.status.as_deref(), Some("Beautify undone"));
    app.undo_beautify(0);
    assert_eq!(app.status.as_deref(), Some("Nothing to undo"), "undo is single use");

    // Editing after Beautify must not be clobbered by Undo.
    app.run_command(Command::BeautifyBody, &ctx);
    app.tabs[0].request.body.push_str("\n// my edit");
    let edited = app.tabs[0].request.body.clone();
    app.undo_beautify(0);
    assert_eq!(app.tabs[0].request.body, edited);
    assert_eq!(app.status.as_deref(), Some("Nothing to undo"));
}

#[test]
fn closing_a_clean_tab_is_immediate_and_a_dirty_tab_asks_first() {
    use postrust::app::CloseChoice;
    use postrust::commands::Command;
    let ctx = egui::Context::default();
    let mut app = App::new(None);
    app.new_tab(Request::default(), None);
    assert_eq!(app.tabs.len(), 2);
    app.run_command(Command::CloseTab, &ctx);
    assert_eq!(app.tabs.len(), 1, "a blank tab closes without asking");
    assert!(app.confirm_close.is_none());

    app.tabs[0].request.url = "http://x".into();
    app.run_command(Command::CloseTab, &ctx);
    assert_eq!(app.tabs.len(), 1, "a dirty tab is kept while the user decides");
    assert_eq!(app.confirm_close, Some(app.tabs[0].id));

    app.resolve_close(CloseChoice::Cancel);
    assert!(app.confirm_close.is_none());
    assert_eq!(app.tabs[0].request.url, "http://x", "cancel keeps the edits");

    app.run_command(Command::CloseTab, &ctx);
    app.resolve_close(CloseChoice::Discard);
    assert_eq!(app.tabs.len(), 1, "closing the last tab leaves a fresh blank one");
    assert_eq!(app.tabs[0].request, Request::default());
    assert!(app.store.collections.is_empty(), "discard saves nothing");

    app.tabs[0].request.url = "http://saved".into();
    app.run_command(Command::CloseTab, &ctx);
    app.resolve_close(CloseChoice::Save);
    assert_eq!(app.store.collections[0].requests[0].url, "http://saved");
    assert_eq!(app.tabs[0].request, Request::default());
}

#[test]
fn resolving_a_close_for_a_tab_that_vanished_is_harmless() {
    use postrust::app::CloseChoice;
    let mut app = App::new(None);
    app.new_tab(Request::new("other"), None);
    app.tabs[1].request.url = "http://x".into();
    app.request_close_tab(1);
    let id = app.confirm_close.unwrap();
    app.close_tab(1); // closed by some other route meanwhile
    assert!(app.tabs.iter().all(|t| t.id != id));
    app.resolve_close(CloseChoice::Discard);
    assert!(app.confirm_close.is_none());
    assert_eq!(app.tabs.len(), 1);
    app.resolve_close(CloseChoice::Discard); // nothing pending: no-op
}

#[test]
fn a_history_tab_is_clean_until_edited_and_closes_without_asking() {
    let mut app = App::new(None);
    let mut from_history = req("old");
    from_history.url = "http://h.test/a".into();
    app.new_tab(from_history, None);
    let idx = app.tabs.len() - 1;
    assert!(
        !app.is_tab_dirty(idx),
        "opening a history entry is not an unsaved change"
    );
    app.request_close_tab(idx);
    assert!(app.confirm_close.is_none(), "no prompt for an untouched tab");
    assert_eq!(app.tabs.len(), 1);

    let mut again = req("old");
    again.url = "http://h.test/a".into();
    app.new_tab(again, None);
    let idx = app.tabs.len() - 1;
    app.tabs[idx].request.body = "edited".into();
    assert!(app.is_tab_dirty(idx));
    app.tabs[idx].request.body.clear();
    assert!(!app.is_tab_dirty(idx), "reverting to how it opened clears the dot");
}

const CURL_POST: &str = r#"curl 'https://api.test/users' -H 'Content-Type: application/json' -H 'Authorization: Bearer tok' -d '{"name":"ada"}' --compressed -F 'f=@x.bin'"#;

#[test]
fn importing_curl_overwrites_the_request_keeps_the_name_and_can_be_undone() {
    let mut app = App::new(None);
    app.tabs[0].request = Request {
        name: "My request".into(),
        method: Method::Delete,
        url: "https://old.test".into(),
        headers: vec![KeyValue::new("X-Old", "1")],
        ..Default::default()
    };
    let before = app.tabs[0].request.clone();

    assert!(app.import_curl(0, CURL_POST));
    let r = &app.tabs[0].request;
    assert_eq!(r.name, "My request", "the name is kept");
    assert_eq!(r.method, Method::Post);
    assert_eq!(r.url, "https://api.test/users");
    assert_eq!(r.body, r#"{"name":"ada"}"#);
    assert_eq!(r.auth, Auth::Bearer { token: "tok".into() });
    assert!(
        r.headers.iter().all(|h| h.key != "X-Old"),
        "the old headers are replaced, not merged"
    );
    let status = app.status.clone().unwrap();
    assert!(status.starts_with("Imported cURL command"), "{status}");
    assert!(status.contains("-F"), "ignored options are reported: {status}");

    app.undo_import(0);
    assert_eq!(app.tabs[0].request, before);
    assert_eq!(app.status.as_deref(), Some("cURL import undone"));
    app.undo_import(0);
    assert_eq!(app.status.as_deref(), Some("Nothing to undo"), "undo is single use");
}

#[test]
fn undo_import_never_discards_edits_made_after_the_import() {
    let mut app = App::new(None);
    assert!(app.import_curl(0, "curl https://a.test/x"));
    app.tabs[0].request.body_kind = BodyKind::Raw;
    app.tabs[0].request.body = "my edit".into();
    let edited = app.tabs[0].request.clone();
    app.undo_import(0);
    assert_eq!(app.tabs[0].request, edited);
    assert_eq!(app.status.as_deref(), Some("Nothing to undo"));
}

#[test]
fn a_failed_import_changes_nothing() {
    let mut app = App::new(None);
    app.tabs[0].request.url = "https://keep.test".into();
    let before = app.tabs[0].request.clone();
    for bad in ["curl 'https://a.test", "curl", "echo hi", "curl https://a.test -H"] {
        assert!(!app.import_curl(0, bad), "{bad}");
        assert_eq!(app.tabs[0].request, before);
        assert!(app.status.as_deref().unwrap().starts_with("Could not import cURL"));
    }
    assert!(app.tabs[0].import_undo.is_none());
    assert!(!app.import_curl(99, "curl https://a.test"), "unknown tab");
}

#[test]
fn an_imported_request_counts_as_unsaved_and_clears_the_beautify_undo() {
    let mut app = App::new(None);
    app.tabs[0].request.url = "https://a.test".into();
    app.save_tab(0);
    assert!(!app.is_tab_dirty(0));
    app.tabs[0].request.body_kind = BodyKind::Raw;
    app.tabs[0].request.body = r#"{"a":1}"#.into();
    app.beautify_body(0);
    assert!(app.tabs[0].beautify_undo.is_some());
    assert!(app.import_curl(0, "curl https://b.test"));
    assert!(app.is_tab_dirty(0), "the saved request differs from the imported one");
    assert!(
        app.tabs[0].beautify_undo.is_none(),
        "the old body is gone, so its undo is too"
    );
}

#[test]
fn importing_discards_the_stale_response_and_cancels_a_request_in_flight() {
    let ctx = egui::Context::default();
    let mut app = App::new(None);
    app.tabs[0].request.url = "http://127.0.0.1:1".into();
    app.send(0, &ctx);
    assert!(app.tabs[0].pending.is_some(), "a request is in flight");
    assert!(app.import_curl(0, "curl https://other.test/x"));
    assert!(app.tabs[0].pending.is_none(), "the old request was cancelled");
    assert!(
        app.tabs[0].response.is_none(),
        "nothing from the old request is shown under the new one"
    );

    // The same goes for Undo: the response of the imported request is not the restored one's.
    app.tabs[0].response = Some(Err("old".into()));
    app.undo_import(0);
    assert!(app.tabs[0].response.is_none());
}
