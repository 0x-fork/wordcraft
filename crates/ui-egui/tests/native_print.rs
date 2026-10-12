//! Native print onboarding preserves engine PDF generation and submits the selected settings.
use serde_json::json;
use std::{cell::RefCell, rc::Rc};
use wordcraft_engine::Session;
use wordcraft_ui_egui::printing::{Duplex, Feedback, Printer};
use wordcraft_ui_egui::{Services, WordApp};

#[test]
fn native_print_uses_selected_settings_and_reports_spool_errors() {
    let mut app = WordApp::new(Session::new(wordcraft_doc::Document::new()), Services::default());
    app.run("text.insert", json!({"text": "A document to print"})).unwrap();
    let jobs = Rc::new(RefCell::new(Vec::new()));
    let observed = jobs.clone();
    app.services.print_native = Some(Box::new(move |bytes, settings| {
        observed.borrow_mut().push((bytes, settings));
        let (sender, receiver) = std::sync::mpsc::channel();
        sender.send(Err("printer is offline".into())).unwrap();
        receiver
    }));
    assert!(app.run("ui.print", json!({})).is_err());
    assert!(jobs.borrow().is_empty(), "opening settings/exporting does not spool");
    app.printing.printers.push(Printer { name: "Chosen queue".into(), default: false });
    let configured = app.run("ui.printSettings", json!({"printer":"Chosen queue", "copies":3,"duplex":"shortEdge","grayscale":true})).unwrap();
    assert_eq!(configured["ready"], true);
    assert_eq!(configured["printers"][0]["name"], "Chosen queue");
    app.run("ui.print", json!({})).unwrap();
    assert!(app.printing.busy());
    assert!(!app.close_requested(), "a pending print job cannot be interrupted by normal close");
    let jobs = jobs.borrow();
    assert_eq!(jobs.len(), 1);
    assert!(jobs[0].0.starts_with(b"%PDF"));
    assert_eq!(jobs[0].1.printer, "Chosen queue");
    assert_eq!(jobs[0].1.copies, 3);
    assert_eq!(jobs[0].1.duplex, Some(Duplex::ShortEdge));
    assert!(jobs[0].1.grayscale);
    app.poll_printing(&egui::Context::default());
    assert_eq!(app.printing.message, Some(Feedback::Failed("printer is offline".into())));
    assert!(app.status_msg.as_ref().unwrap().0.contains("printer is offline"));
}

#[test]
fn print_settings_are_visible_in_a_narrow_window() {
    use egui_kittest::kittest::{NodeT, Queryable};
    let mut app = WordApp::new(Session::new(wordcraft_doc::Document::new()), Services::default());
    app.run("text.insert", json!({"text": "A document to print"})).unwrap();
    app.services.printers = Some(Box::new(|| {
        let (sender, receiver) = std::sync::mpsc::channel();
        sender.send(vec![Printer { name: "Test queue".into(), default: true }]).unwrap();
        receiver
    }));
    app.services.print_native = Some(Box::new(|_, _| {
        let (sender, receiver) = std::sync::mpsc::channel();
        sender.send(Ok("queued".into())).unwrap();
        receiver
    }));
    app.ui.backstage = true;
    app.ui.backstage_page = "print".into();
    let mut harness = egui_kittest::Harness::builder().with_size(egui::vec2(800.0, 700.0)).build_ui_state(
        |ui, app: &mut WordApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    harness.run_steps(5);
    assert!(!harness.get_by_label("Refresh printers").accesskit_node().is_disabled());
    assert!(!harness.get_by_label("Grayscale").accesskit_node().is_disabled());
    harness.get_by_label("Grayscale").click();
    harness.run_steps(2);
    assert!(harness.state().printing.settings.grayscale);
    assert!(harness.state().printing.ready());
}

#[test]
fn invalid_print_settings_are_rejected_without_partial_changes() {
    let mut app = WordApp::new(Session::new(wordcraft_doc::Document::new()), Services::default());
    app.printing.printers.push(Printer { name: "Queue".into(), default: true });
    let before = app.run("ui.printSettings", json!({})).unwrap();
    for params in [
        json!({"printer":"Missing"}),
        json!({"copies":0}),
        json!({"copies":1000}),
        json!({"copies":1.5}),
        json!({"copies":-1}),
        json!({"copies":"3"}),
        json!({"duplex":"maybe"}),
        json!({"grayscale":"yes"}),
        json!({"refresh":"yes"}),
        json!({"copies":3,"duplex":"invalid"}),
        json!([]),
    ] {
        assert!(app.run("ui.printSettings", params.clone()).is_err(), "{params}");
        assert_eq!(app.run("ui.printSettings", json!({})).unwrap(), before, "no partial update: {params}");
    }
}

#[test]
fn repeated_fast_jobs_each_surface_an_identical_receipt() {
    let mut app = WordApp::new(Session::new(wordcraft_doc::Document::new()), Services::default());
    app.printing.printers.push(Printer { name: "Queue".into(), default: true });
    app.run("ui.printSettings", json!({"printer":"Queue"})).unwrap();
    app.services.print_native = Some(Box::new(|_, _| {
        let (sender, receiver) = std::sync::mpsc::channel();
        sender.send(Ok("same receipt".into())).unwrap();
        receiver
    }));
    for _ in 0..2 {
        app.status_msg = None;
        app.run("ui.printSubmit", json!({})).unwrap();
        // Completion may be consumed by the shared widget before the app's logic pass.
        app.printing.poll(&egui::Context::default());
        app.poll_printing(&egui::Context::default());
        assert!(app.status_msg.as_ref().unwrap().0.contains("same receipt"));
        app.status_msg = None;
        app.poll_printing(&egui::Context::default());
        assert!(app.status_msg.is_none());
    }
}
