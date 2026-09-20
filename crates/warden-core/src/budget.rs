use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use crate::model::Usage;

/// What one turn is allowed to spend on sub-agents (P46/P60), shared by the whole tree of
/// orchestrators a turn spawns (`delegate_task`, `delegate_to_agent`, and whatever those delegate
/// to in turn) — one budget per turn, however deep the tree goes.
///
/// It counts model calls made by *sub-agents* only. The orchestrator the turn started in is never
/// charged: it is already bounded by `MAX_TOOL_ITERATIONS`, and leaving it out means that once the
/// budget runs out the caller can still answer with what its sub-agents produced (their refusal
/// reaches it as an ordinary tool error) instead of the whole turn failing.
///
/// It also adds up the tokens those calls report, so the turn's `MessageOutcome.usage` includes
/// work that `Tool::call`'s plain JSON result can't carry back (P18).
#[derive(Debug)]
pub struct TurnBudget {
    max_calls: u32,
    calls: AtomicU32,
    usage: Mutex<Option<Usage>>,
}

impl TurnBudget {
    pub fn new(max_calls: u32) -> Arc<Self> {
        Arc::new(Self { max_calls, calls: AtomicU32::new(0), usage: Mutex::new(None) })
    }

    /// Counts one sub-agent model call, or says why it can't be made. The refused call is not
    /// counted, so `calls()` never exceeds the limit.
    pub fn charge(&self) -> anyhow::Result<()> {
        let taken = self
            .calls
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| (n < self.max_calls).then_some(n + 1))
            .is_ok();
        if taken {
            Ok(())
        } else {
            anyhow::bail!(
                "this turn's limit of {} model calls by sub-agents was reached — stop delegating and answer with what you already have",
                self.max_calls
            )
        }
    }

    /// Adds one sub-agent call's reported usage. A provider that reports none adds nothing.
    pub fn record(&self, usage: Option<&Usage>) {
        if let Some(usage) = usage {
            *self.usage.lock().unwrap().get_or_insert_with(Usage::default) += usage;
        }
    }

    /// Sub-agent model calls made so far.
    pub fn calls(&self) -> u32 {
        self.calls.load(Ordering::SeqCst)
    }

    /// Tokens reported by sub-agents so far; `None` when none was reported.
    pub fn usage(&self) -> Option<Usage> {
        *self.usage.lock().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charges_up_to_the_limit_then_refuses_without_counting() {
        let budget = TurnBudget::new(2);
        assert!(budget.charge().is_ok() && budget.charge().is_ok());
        let err = budget.charge().unwrap_err().to_string();
        assert!(err.contains("limit of 2 model calls"), "{err}");
        assert!(budget.charge().is_err());
        assert_eq!(budget.calls(), 2);
    }

    #[test]
    fn a_zero_limit_refuses_everything() {
        assert!(TurnBudget::new(0).charge().is_err());
    }

    #[test]
    fn records_usage_only_when_reported() {
        let budget = TurnBudget::new(5);
        assert_eq!(budget.usage(), None);
        budget.record(None);
        assert_eq!(budget.usage(), None);
        budget.record(Some(&Usage { prompt_tokens: 3, completion_tokens: 2, total_tokens: 5 }));
        budget.record(Some(&Usage { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 }));
        assert_eq!(budget.usage(), Some(Usage { prompt_tokens: 4, completion_tokens: 3, total_tokens: 7 }));
    }
}
