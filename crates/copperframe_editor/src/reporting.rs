//! Crash prompt and "Report a bug" dialog.
//!
//! Nothing is sent from here. With the user's go-ahead the editor opens a
//! pre-filled email draft in their own mail program
//! ([`copperframe_core::report::mailto_url`]); they review it and press Send
//! there. The report text is shown in the dialog first.

use copperframe_core::report::{self, CrashReport};

use crate::session::EditorSession;

/// Dialog width in points; long report lines scroll instead of widening it.
const REPORT_WIDTH: f32 = 620.0;

/// State of the two dialogs.
#[derive(Default)]
pub struct Reporting {
    /// Unseen crash reports, newest first. The prompt shows while non-empty.
    crashes: Vec<CrashReport>,
    bug: Option<BugDraft>,
}

struct BugDraft {
    summary: String,
    description: String,
    include_log: bool,
}

impl Reporting {
    /// Looks for crash reports the user has not acted on (from an earlier
    /// editor run or a Play session that just crashed).
    pub fn check_for_crashes(&mut self) {
        self.crashes = report::pending_reports();
    }

    pub fn open_bug_report(&mut self) {
        if self.bug.is_none() {
            self.bug = Some(BugDraft {
                summary: String::new(),
                description: String::new(),
                include_log: true,
            });
        }
    }

    /// Whether a dialog is showing.
    pub fn is_open(&self) -> bool {
        !self.crashes.is_empty() || self.bug.is_some()
    }

    pub fn show(&mut self, ctx: &egui::Context, session: &mut EditorSession) {
        self.crash_prompt(ctx, session);
        self.bug_dialog(ctx, session);
    }

    fn crash_prompt(&mut self, ctx: &egui::Context, session: &mut EditorSession) {
        let Some(newest) = self.crashes.first().cloned() else {
            return;
        };
        let email = report::report_email();
        let mut done = false;
        egui::Modal::new(egui::Id::unique("copperframe-crash-report")).show(ctx, |ui| {
            ui.set_width(REPORT_WIDTH);
            ui.heading("Copperframe closed unexpectedly");
            ui.label(
                "A crash report was saved on this computer. Nothing has been sent. \
                 You can email it so the problem can be fixed: your mail program opens \
                 with a draft you can read and edit before sending.",
            );
            if self.crashes.len() > 1 {
                ui.label(format!(
                    "{} reports are waiting; this is the newest.",
                    self.crashes.len()
                ));
            }
            ui.add(
                egui::Label::new(egui::RichText::new(newest.path.display().to_string()).weak())
                    .wrap(),
            );
            egui::ScrollArea::both()
                .max_height(260.0)
                .max_width(REPORT_WIDTH)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(&newest.text).monospace())
                            .wrap_mode(egui::TextWrapMode::Extend),
                    );
                });
            ui.horizontal(|ui| {
                if let Some(email) = email
                    && ui
                        .button("Email report…")
                        .on_hover_text(format!("Opens a draft to {email} in your mail program"))
                        .clicked()
                {
                    open(session, &report::crash_mail(email, &newest), "email draft");
                    session.info(format!(
                        "opened an email draft; if it is cut short, attach {}",
                        newest.path.display()
                    ));
                    done = true;
                }
                if ui.button("Copy").clicked() {
                    ctx.copy_text(newest.text.clone());
                }
                if ui.button("Open folder").clicked()
                    && let Some(dir) = newest.path.parent()
                {
                    open(session, &dir.display().to_string(), "report folder");
                }
                if ui.button("Dismiss").clicked() {
                    done = true;
                }
            });
        });
        if done {
            // Older reports are usually the same problem; one prompt is enough.
            for r in &self.crashes {
                report::mark_seen(r);
            }
            self.crashes.clear();
        }
    }

    fn bug_dialog(&mut self, ctx: &egui::Context, session: &mut EditorSession) {
        let Some(draft) = self.bug.as_mut() else {
            return;
        };
        let mut close = false;
        egui::Modal::new(egui::Id::unique("copperframe-bug-report")).show(ctx, |ui| {
            ui.set_width(REPORT_WIDTH);
            ui.heading("Report a bug");
            ui.label(
                "Describe the problem. Your mail program opens with a draft you can \
                 edit before sending; nothing is sent from the editor.",
            );
            ui.add(
                egui::TextEdit::singleline(&mut draft.summary)
                    .hint_text("Short summary")
                    .desired_width(f32::INFINITY),
            );
            ui.add(
                egui::TextEdit::multiline(&mut draft.description)
                    .hint_text("What happened, what you expected, and the steps to make it happen")
                    .desired_rows(6)
                    .desired_width(f32::INFINITY),
            );
            ui.checkbox(
                &mut draft.include_log,
                "Include system details and recent log lines",
            );
            egui::CollapsingHeader::new("What will be in the email")
                .default_open(false)
                .show(ui, |ui| {
                    let body = report::bug_body(&draft.description, draft.include_log);
                    egui::ScrollArea::both()
                        .max_height(200.0)
                        .max_width(REPORT_WIDTH)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(egui::RichText::new(body).monospace())
                                    .wrap_mode(egui::TextWrapMode::Extend),
                            );
                        });
                });
            ui.horizontal(|ui| {
                match report::report_email() {
                    Some(email) => {
                        if ui.button("Open email draft").clicked() {
                            let url = report::bug_mail(
                                email,
                                &draft.summary,
                                &draft.description,
                                draft.include_log,
                            );
                            open(session, &url, "email draft");
                            close = true;
                        }
                    }
                    None => {
                        ui.label("No report address is configured.");
                    }
                }
                if ui.button("Copy").clicked() {
                    ctx.copy_text(report::bug_body(&draft.description, draft.include_log));
                }
                if let Some(dir) = report::logs_dir()
                    && ui
                        .button("Open log folder")
                        .on_hover_text("Attach the .log files for more detail")
                        .clicked()
                {
                    let _ = std::fs::create_dir_all(&dir);
                    open(session, &dir.display().to_string(), "log folder");
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close {
            self.bug = None;
        }
    }
}

fn open(session: &mut EditorSession, target: &str, what: &str) {
    if let Err(e) = report::open_external(target) {
        session.error(format!("could not open the {what}: {e}"));
    }
}
