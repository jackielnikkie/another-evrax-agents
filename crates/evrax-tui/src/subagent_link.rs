use std::fs;

use evrax_agent::subagent;

use crate::app::App;

pub struct Link {
    state: subagent::State,
    last_status: String,
    last_reply: String,
}

impl Link {
    pub fn load(id: &str) -> Option<Self> {
        let state = subagent::load(id)?;
        Some(Self {
            last_status: state.status.clone(),
            last_reply: state.last_reply.clone(),
            state,
        })
    }

    pub fn name(&self) -> &str {
        &self.state.name
    }

    pub fn take_prompt(&self) -> Option<String> {
        let path = subagent::inbox_path(&self.state.id);
        let text = fs::read_to_string(&path).ok()?;
        let _ = fs::remove_file(&path);
        let text = text.trim().to_string();
        (!text.is_empty()).then_some(text)
    }

    pub fn stop_requested(&self) -> bool {
        subagent::stop_path(&self.state.id).exists()
    }

    pub fn publish(&mut self, app: &App) {
        self.publish_as(app, if app.busy { "busy" } else { "idle" });
    }

    pub fn publish_final(&mut self, app: &App, status: &str) {
        self.publish_as(app, status);
    }

    fn publish_as(&mut self, app: &App, status: &str) {
        let last_reply = app.last_assistant_text().unwrap_or_default();
        let pid = std::process::id();
        if self.state.status == status
            && self.state.pid == pid
            && self.state.last_reply == last_reply
            && self.last_status == status
            && self.last_reply == last_reply
        {
            return;
        }
        self.state.pid = pid;
        self.state.status = status.to_string();
        self.state.updated_ms = subagent::now_ms();
        self.state.last_reply = last_reply.clone();
        self.last_status = status.to_string();
        self.last_reply = last_reply;
        let _ = subagent::save(&self.state);
    }
}
