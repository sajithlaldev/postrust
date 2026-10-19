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
