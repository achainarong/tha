//! Bounded, explicitly selected PTY sessions. Selection never changes a session's target.
use crate::{exec::Request, terminal::Session};
use anyhow::{Context, Result, bail};
use std::sync::Arc;
use tokio::sync::Notify;

struct Slot {
    id: u64,
    terminal: Session,
}
pub struct Sessions {
    entries: Vec<Slot>,
    active: Option<u64>,
    next_id: u64,
    limit: usize,
    output_ready: Arc<Notify>,
}
pub struct Notice {
    pub id: u64,
    pub status: String,
}

impl Default for Sessions {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            active: None,
            next_id: 1,
            limit: 4,
            output_ready: Arc::default(),
        }
    }
}
impl Sessions {
    /// Signalled when any session has new output, so its echo need not wait for the next tick.
    pub fn output_ready(&self) -> Arc<Notify> {
        self.output_ready.clone()
    }
    /// Whether the selected session printed anything since the last call.
    pub fn take_active_output_changed(&mut self) -> bool {
        self.active_mut().is_some_and(Session::take_output_changed)
    }
    pub fn set_limit(&mut self, limit: usize) -> Result<()> {
        if !(1..=8).contains(&limit) || self.entries.len() > limit {
            bail!(crate::i18n::t(
                "Exec session limit must be 1–8; close retained sessions before reducing it"
            ));
        }
        self.limit = limit;
        Ok(())
    }
    pub fn limit(&self) -> usize {
        self.limit
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn active_id(&self) -> Option<u64> {
        self.active
    }
    pub fn active(&self) -> Option<&Session> {
        let id = self.active?;
        self.entries
            .iter()
            .find(|slot| slot.id == id)
            .map(|slot| &slot.terminal)
    }
    pub fn active_mut(&mut self) -> Option<&mut Session> {
        let id = self.active?;
        self.entries
            .iter_mut()
            .find(|slot| slot.id == id)
            .map(|slot| &mut slot.terminal)
    }
    pub fn iter(&self) -> impl Iterator<Item = (u64, &Session)> {
        self.entries.iter().map(|slot| (slot.id, &slot.terminal))
    }
    pub fn select(&mut self, id: u64) -> bool {
        if self.entries.iter().any(|slot| slot.id == id) {
            self.active = Some(id);
            true
        } else {
            false
        }
    }
    pub fn open(&mut self, request: Request, dimensions: (u16, u16)) -> Result<u64> {
        let _ = self.poll();
        if let Some(slot) = self
            .entries
            .iter_mut()
            .find(|slot| slot.terminal.running() && slot.terminal.same_target(&request))
        {
            self.active = Some(slot.id);
            slot.terminal.follow();
            return Ok(slot.id);
        }
        if self.entries.len() >= self.limit {
            bail!(crate::tf!(
                "Exec session limit reached ({open}/{limit} retained). Use F6 to select one and :close-exec to free a slot",
                open = self.len(),
                limit = self.limit
            ));
        }
        let command = crate::exec::prepare(&request)?;
        let session = Session::start(
            request,
            command,
            dimensions.0,
            dimensions.1,
            self.output_ready.clone(),
        )?;
        self.insert(session)
    }
    /// A command on this machine that needs a terminal, such as `ssh` asking about a host key.
    pub fn open_local_tool(
        &mut self,
        title: &str,
        kind: &str,
        command: std::process::Command,
        dimensions: (u16, u16),
    ) -> Result<u64> {
        let _ = self.poll();
        let request = crate::machines::local_tool_request(title, kind);
        if kind == "server-shell"
            && let Some(slot) = self.entries.iter_mut().find(|slot| {
                slot.terminal.running()
                    && slot.terminal.same_target(&request)
                    && slot.terminal.request.container.name == title
            })
        {
            self.active = Some(slot.id);
            slot.terminal.follow();
            return Ok(slot.id);
        }
        if self.entries.len() >= self.limit {
            bail!(crate::tf!(
                "Exec session limit reached ({open}/{limit} retained). Use F6 to select one and :close-exec to free a slot",
                open = self.len(),
                limit = self.limit
            ));
        }
        let session = Session::start(
            request,
            command,
            dimensions.0,
            dimensions.1,
            self.output_ready.clone(),
        )?;
        self.insert(session)
    }
    fn insert(&mut self, terminal: Session) -> Result<u64> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .context(crate::i18n::t("Exec session IDs exhausted"))?;
        self.entries.push(Slot { id, terminal });
        self.active = Some(id);
        Ok(id)
    }
    pub fn close_active(&mut self) {
        let Some(id) = self.active.take() else {
            return;
        };
        if let Some(index) = self.entries.iter().position(|slot| slot.id == id) {
            self.entries.remove(index).terminal.close();
            self.active = self
                .entries
                .get(index.min(self.entries.len().saturating_sub(1)))
                .map(|slot| slot.id);
        }
    }
    pub fn poll(&mut self) -> Vec<Notice> {
        self.entries
            .iter_mut()
            .filter_map(|slot| {
                if slot.terminal.poll() {
                    Some(Notice {
                        id: slot.id,
                        status: slot.terminal.status(),
                    })
                } else {
                    None
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(id: &str) -> Request {
        Request {
            connection: crate::config::Connection {
                name: "mock".into(),
                kind: crate::config::BackendKind::Docker,
                endpoint: "unix:///mock-only".into(),
            },
            container: crate::model::Container {
                id: id.into(),
                name: id.into(),
                state: "running".into(),
                ..Default::default()
            },
            command: vec!["sh".into()],
        }
    }
    fn mock(id: &str) -> Session {
        let mut command = std::process::Command::new("/bin/sh");
        command.args(["-c", "exec sleep 30"]);
        Session::start(request(id), command, 20, 70, Arc::default()).unwrap()
    }
    #[test]
    fn sessions_keep_ids_targets_and_selection_independent_and_never_evict_at_the_limit() {
        let mut sessions = Sessions::default();
        sessions.set_limit(2).unwrap();
        let first = sessions.insert(mock("first")).unwrap();
        let second = sessions.insert(mock("second")).unwrap();
        assert!(sessions.select(first));
        assert_eq!(sessions.active().unwrap().request.container.id, "first");
        assert_eq!(sessions.open(request("first"), (20, 70)).unwrap(), first);
        assert_eq!(sessions.len(), 2);
        assert!(sessions.open(request("third"), (20, 70)).is_err());
        assert_eq!(sessions.active_id(), Some(first));
        assert!(!sessions.select(999));
        assert_eq!(sessions.active_id(), Some(first));
        sessions.close_active();
        assert_eq!(sessions.active_id(), Some(second));
        assert_eq!(sessions.len(), 1);
        let third = sessions.insert(mock("third")).unwrap();
        assert!(third > second);
        assert_eq!(sessions.active().unwrap().request.container.id, "third");
    }
    #[tokio::test]
    async fn new_output_wakes_the_loop_and_marks_only_the_selected_session_changed() {
        let mut sessions = Sessions::default();
        let ready = sessions.output_ready();
        let background = sessions.insert(mock("background")).unwrap();
        let mut command = std::process::Command::new("/bin/sh");
        command.args(["-c", "printf ready; exec sleep 30"]);
        let printing = Session::start(request("printing"), command, 20, 70, ready.clone()).unwrap();
        let active = sessions.insert(printing).unwrap();
        assert_ne!(background, active);
        assert!(!sessions.take_active_output_changed(), "nothing polled yet");

        tokio::time::timeout(std::time::Duration::from_secs(5), ready.notified())
            .await
            .expect("output signals without waiting for a tick");
        sessions.poll();
        assert!(sessions.take_active_output_changed());
        assert!(!sessions.take_active_output_changed(), "reported once");
        assert!(
            sessions
                .active()
                .unwrap()
                .screen()
                .contents()
                .contains("ready")
        );

        sessions.poll();
        assert!(
            !sessions.take_active_output_changed(),
            "no output, no redraw"
        );
    }
}
