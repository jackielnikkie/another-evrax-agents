use std::collections::VecDeque;
use std::time::Duration;

use futures_util::StreamExt;
use futures_util::stream::{self, BoxStream};
use tokio::sync::mpsc;

#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    ThinkingDelta { text: String },
    ThinkingFinished { seconds: f32 },
    TextDelta { text: String },
    ToolStarted { name: String, summary: String },
    ToolFinished { name: String, ok: bool, detail: String },
    ApprovalRequested { name: String, summary: String },
    Diff { path: String, lines: Vec<DiffLine> },
    TurnFinished,
    Failed { message: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    Add,
    Rem,
    Ctx,
}

pub trait AgentBackend: Send + Sync {
    /// Runs one turn. `decisions` carries tool-approval answers (true = allow)
    /// in the order `ApprovalRequested` events are emitted.
    fn run_turn(&self, prompt: String, decisions: mpsc::Receiver<bool>) -> BoxStream<'static, AgentEvent>;
}

pub struct MockBackend {
    pace: Duration,
}

impl MockBackend {
    pub fn new() -> Self {
        Self {
            pace: Duration::from_millis(45),
        }
    }

    pub fn with_pace(pace: Duration) -> Self {
        Self { pace }
    }
}

impl Default for MockBackend {
    fn default() -> Self {
        Self::new()
    }
}

enum Step {
    Event(AgentEvent),
    Approval {
        name: String,
        summary: String,
        allow: Vec<Step>,
        reject: Vec<Step>,
    },
}

impl AgentBackend for MockBackend {
    fn run_turn(
        &self,
        prompt: String,
        decisions: mpsc::Receiver<bool>,
    ) -> BoxStream<'static, AgentEvent> {
        let pace = self.pace;
        let steps = mock_script(prompt);
        let state = (steps, decisions, None::<(Vec<Step>, Vec<Step>)>);
        stream::unfold(state, move |(mut steps, mut decisions, mut branches)| async move {
            loop {
                if let Some((allow_steps, reject_steps)) = branches.take() {
                    let allow = decisions.recv().await.unwrap_or(false);
                    let chosen = if allow { allow_steps } else { reject_steps };
                    for step in chosen.into_iter().rev() {
                        steps.push_front(step);
                    }
                    continue;
                }
                let step = steps.pop_front()?;
                if !pace.is_zero() {
                    tokio::time::sleep(pace).await;
                }
                match step {
                    Step::Event(event) => return Some((event, (steps, decisions, branches))),
                    Step::Approval {
                        name,
                        summary,
                        allow,
                        reject,
                    } => {
                        let event = AgentEvent::ApprovalRequested { name, summary };
                        return Some((event, (steps, decisions, Some((allow, reject)))));
                    }
                }
            }
        })
        .boxed()
    }
}

fn mock_script(prompt: String) -> VecDeque<Step> {
    let target = "crates/evrax-tui/src/ui.rs";
    let mut steps: VecDeque<Step> = VecDeque::new();

    push_words(
        &mut steps,
        |text| AgentEvent::ThinkingDelta { text },
        "scanning the workspace, planning the smallest safe change ",
    );
    steps.push_back(Step::Event(AgentEvent::ThinkingFinished { seconds: 1.8 }));

    let intro = format!("On it: {prompt}\n\nReading the target file first. ");
    push_words(&mut steps, |text| AgentEvent::TextDelta { text }, &intro);

    steps.push_back(Step::Event(AgentEvent::ToolStarted {
        name: "read".to_string(),
        summary: target.to_string(),
    }));
    steps.push_back(Step::Event(AgentEvent::ToolFinished {
        name: "read".to_string(),
        ok: true,
        detail: "145 lines · centered card in body()".to_string(),
    }));

    steps.push_back(Step::Event(AgentEvent::Diff {
        path: target.to_string(),
        lines: vec![
            DiffLine {
                kind: DiffKind::Rem,
                text: "let width = 46.min(area.width);".to_string(),
            },
            DiffLine {
                kind: DiffKind::Add,
                text: "let columns = Layout::horizontal([".to_string(),
            },
            DiffLine {
                kind: DiffKind::Add,
                text: "    Constraint::Min(0),".to_string(),
            },
            DiffLine {
                kind: DiffKind::Add,
                text: "    Constraint::Length(SIDEBAR_WIDTH),".to_string(),
            },
            DiffLine {
                kind: DiffKind::Add,
                text: "]);".to_string(),
            },
            DiffLine {
                kind: DiffKind::Ctx,
                text: "frame.render_widget(block, card);".to_string(),
            },
        ],
    }));

    let mut allow: Vec<Step> = Vec::new();
    allow.push(Step::Event(AgentEvent::ToolStarted {
        name: "edit".to_string(),
        summary: target.to_string(),
    }));
    allow.push(Step::Event(AgentEvent::ToolFinished {
        name: "edit".to_string(),
        ok: true,
        detail: "1 hunk applied".to_string(),
    }));
    push_words(
        &mut allow,
        |text| AgentEvent::TextDelta { text },
        "**Edit applied.** Running the test suite to verify. ",
    );
    allow.push(Step::Event(AgentEvent::ToolStarted {
        name: "shell".to_string(),
        summary: "cargo test".to_string(),
    }));
    allow.push(Step::Event(AgentEvent::ToolFinished {
        name: "shell".to_string(),
        ok: true,
        detail: "5 passed · 0.42s".to_string(),
    }));
    push_words(
        &mut allow,
        |text| AgentEvent::TextDelta { text },
        "Done — **all tests green**:\n\n- edit applied cleanly\n- `cargo test` passes (5/5)\n- nothing else touched ",
    );
    allow.push(Step::Event(AgentEvent::TurnFinished));

    let mut reject: Vec<Step> = Vec::new();
    reject.push(Step::Event(AgentEvent::ToolStarted {
        name: "edit".to_string(),
        summary: target.to_string(),
    }));
    reject.push(Step::Event(AgentEvent::ToolFinished {
        name: "edit".to_string(),
        ok: false,
        detail: "rejected by user".to_string(),
    }));
    push_words(
        &mut reject,
        |text| AgentEvent::TextDelta { text },
        "Left the file *untouched* — stopping here. ",
    );
    reject.push(Step::Event(AgentEvent::TurnFinished));

    steps.push_back(Step::Approval {
        name: "edit".to_string(),
        summary: target.to_string(),
        allow,
        reject,
    });
    steps
}

fn push_words<F>(steps: &mut impl Extend<Step>, make: F, text: &str)
where
    F: Fn(String) -> AgentEvent,
{
    for word in text.split_inclusive(' ') {
        steps.extend(std::iter::once(Step::Event(make(word.to_string()))));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;

    async fn run_with_decision(prompt: &str, decision: Option<bool>) -> Vec<AgentEvent> {
        let backend = MockBackend::with_pace(Duration::ZERO);
        let (tx, rx) = mpsc::channel(1);
        let mut stream = backend.run_turn(prompt.to_string(), rx);
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            let is_approval = matches!(event, AgentEvent::ApprovalRequested { .. });
            events.push(event);
            if is_approval {
                tx.send(decision.unwrap_or(true)).await.unwrap();
            }
        }
        events
    }

    #[tokio::test]
    async fn mock_stream_ends_with_turn_finished() {
        let events = run_with_decision("fix the layout", Some(true)).await;
        assert_eq!(events.last(), Some(&AgentEvent::TurnFinished));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::ThinkingDelta { .. }))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::Diff { .. }))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::ApprovalRequested { .. }))
        );
        assert!(!events.iter().any(|e| matches!(
            e,
            AgentEvent::ToolFinished { ok: false, .. }
        )));
    }

    #[tokio::test]
    async fn mock_stream_pairs_tool_calls() {
        let events = run_with_decision("x", Some(true)).await;
        let started = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ToolStarted { .. }))
            .count();
        let finished = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ToolFinished { .. }))
            .count();
        assert_eq!(started, finished);
        assert!(started >= 3);
    }

    #[tokio::test]
    async fn approval_allow_continues_turn() {
        let events = run_with_decision("fix", Some(true)).await;
        assert!(events.iter().any(
            |e| matches!(e, AgentEvent::ToolFinished { name, ok: true, .. } if name == "edit")
        ));
        assert!(events.iter().any(
            |e| matches!(e, AgentEvent::ToolFinished { name, ok: true, .. } if name == "shell")
        ));
        assert_eq!(events.last(), Some(&AgentEvent::TurnFinished));
    }

    #[tokio::test]
    async fn approval_reject_stops_edit_and_finishes() {
        let events = run_with_decision("fix", Some(false)).await;
        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::ToolFinished { name, ok: false, detail }
                if name == "edit" && detail == "rejected by user"
        )));
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, AgentEvent::ToolStarted { name, .. } if name == "shell"))
        );
        assert_eq!(events.last(), Some(&AgentEvent::TurnFinished));
    }

    #[test]
    fn backend_is_object_safe() {
        let backend: Box<dyn AgentBackend> = Box::new(MockBackend::with_pace(Duration::ZERO));
        let (_tx, rx) = mpsc::channel(1);
        let mut stream = backend.run_turn("hello".into(), rx);
        assert!(stream.next().now_or_never().is_some());
    }
}
