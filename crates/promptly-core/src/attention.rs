//! The attention queue: sessions that need the human, ranked by urgency then
//! wait time, plus the dedup/rate-limit policy for native notifications.

use crate::state::SessionState;
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub type SessionKey = u64;

#[derive(Debug, Clone)]
pub struct AttentionItem {
    pub session: SessionKey,
    pub state: SessionState,
    pub since: Instant,
    pub detail: Option<String>,
    pub priority: i8,
}

#[derive(Debug, Default)]
pub struct AttentionQueue {
    items: HashMap<SessionKey, AttentionItem>,
}

impl AttentionQueue {
    /// Record a session's current state; non-attention states leave the queue.
    pub fn update(&mut self, item: AttentionItem) {
        if item.state.needs_attention() {
            self.items.insert(item.session, item);
        } else {
            self.items.remove(&item.session);
        }
    }

    pub fn remove(&mut self, s: SessionKey) {
        self.items.remove(&s);
    }

    /// Most urgent first; ties broken by the longest wait.
    pub fn ranked(&self) -> Vec<&AttentionItem> {
        let mut v: Vec<_> = self.items.values().collect();
        v.sort_by(|a, b| {
            (b.state.urgency() as i16 + b.priority as i16)
                .cmp(&(a.state.urgency() as i16 + a.priority as i16))
                .then(a.since.cmp(&b.since))
        });
        v
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// Decides whether a transition deserves a native notification.
#[derive(Debug)]
pub struct NotifyPolicy {
    last: HashMap<(SessionKey, SessionState), Instant>,
    window_start: Instant,
    in_window: u32,
    pub per_session_cooldown: Duration,
    pub max_per_minute: u32,
    pub notify_on_finish: bool,
}

impl Default for NotifyPolicy {
    fn default() -> Self {
        Self {
            last: HashMap::new(),
            window_start: Instant::now(),
            in_window: 0,
            per_session_cooldown: Duration::from_secs(20),
            max_per_minute: 6,
            notify_on_finish: true,
        }
    }
}

impl NotifyPolicy {
    /// `focused` is true when the user is already looking at the session, in
    /// which case a notification would only be noise.
    pub fn should_notify(
        &mut self,
        s: SessionKey,
        to: SessionState,
        muted: bool,
        focused: bool,
    ) -> bool {
        let blocking = matches!(
            to,
            SessionState::WaitingPermission | SessionState::WaitingInput | SessionState::Error
        );
        let finish = self.notify_on_finish && matches!(to, SessionState::Done | SessionState::Idle);
        if muted || focused || !(blocking || finish) {
            return false;
        }
        let now = Instant::now();
        if let Some(t) = self.last.get(&(s, to))
            && now.duration_since(*t) < self.per_session_cooldown
        {
            return false;
        }
        if now.duration_since(self.window_start) > Duration::from_secs(60) {
            self.window_start = now;
            self.in_window = 0;
        }
        // Permission prompts always get through; everything else is rate limited.
        if to != SessionState::WaitingPermission && self.in_window >= self.max_per_minute {
            return false;
        }
        self.in_window += 1;
        self.last.insert((s, to), now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(s: SessionKey, st: SessionState, ago: u64) -> AttentionItem {
        AttentionItem {
            session: s,
            state: st,
            since: Instant::now() - Duration::from_secs(ago),
            detail: None,
            priority: 0,
        }
    }

    #[test]
    fn ranking() {
        let mut q = AttentionQueue::default();
        q.update(item(1, SessionState::Done, 100));
        q.update(item(2, SessionState::WaitingPermission, 1));
        q.update(item(3, SessionState::WaitingPermission, 50));
        q.update(item(4, SessionState::Working, 500));
        let order: Vec<_> = q.ranked().iter().map(|i| i.session).collect();
        assert_eq!(order, vec![3, 2, 1]);
        q.update(item(3, SessionState::Working, 0));
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn dedup_and_focus() {
        let mut p = NotifyPolicy::default();
        assert!(p.should_notify(1, SessionState::WaitingPermission, false, false));
        assert!(
            !p.should_notify(1, SessionState::WaitingPermission, false, false),
            "dedup"
        );
        assert!(
            !p.should_notify(2, SessionState::WaitingInput, false, true),
            "focused"
        );
        assert!(
            !p.should_notify(2, SessionState::WaitingInput, true, false),
            "muted"
        );
        assert!(
            !p.should_notify(2, SessionState::Working, false, false),
            "not blocking"
        );
    }
}
