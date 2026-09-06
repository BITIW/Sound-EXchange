//! Sample-domain-independent finite/causal scheduling and indexed FIR history.
//! The evaluator receives only the requested phase and its exact input window.

use super::{
    FrameCountPolicy, PhaseClock, PolyphaseError, RateRatio, StreamError, StreamStats,
    output_frames_for_input,
};
use sexq::ArithmeticOutcome;
use std::collections::VecDeque;

#[derive(Debug)]
pub(crate) struct Timeline<S> {
    clock: PhaseClock,
    taps: usize,
    zero: S,
    history: VecDeque<S>,
    history_start_index: u64,
    scratch: Vec<S>,
    timeline_frames: u64,
    stats: StreamStats,
    output_limit: Option<u64>,
    finite_policy: Option<FrameCountPolicy>,
    poisoned: bool,
}

impl<S: Clone> Timeline<S> {
    pub(crate) fn new(
        ratio: RateRatio,
        delay: u64,
        taps: usize,
        zero: S,
    ) -> Result<Self, StreamError> {
        if taps == 0 {
            return Err(PolyphaseError::ZeroTaps.into());
        }
        let mut history = VecDeque::new();
        history
            .try_reserve(taps)
            .map_err(|_| StreamError::AllocationFailed)?;
        let mut scratch = Vec::new();
        scratch
            .try_reserve(taps)
            .map_err(|_| StreamError::AllocationFailed)?;
        Ok(Self {
            clock: PhaseClock::with_input_offset(ratio, delay),
            taps,
            zero,
            history,
            history_start_index: 0,
            scratch,
            timeline_frames: 0,
            stats: StreamStats::default(),
            output_limit: None,
            finite_policy: None,
            poisoned: false,
        })
    }

    pub(crate) const fn stats(&self) -> StreamStats {
        self.stats
    }
    pub(crate) fn retained_frames(&self) -> usize {
        self.history.len()
    }
    pub(crate) fn poison(&mut self) {
        self.poisoned = true;
    }
    pub(crate) fn ensure_ready(&self) -> Result<(), StreamError> {
        if self.poisoned {
            Err(StreamError::Poisoned)
        } else {
            Ok(())
        }
    }

    pub(crate) fn push<T>(
        &mut self,
        input: &[S],
        policy: Option<FrameCountPolicy>,
        output: &mut Vec<T>,
        mut evaluate: impl FnMut(u64, &[S]) -> Result<ArithmeticOutcome<T>, PolyphaseError>,
    ) -> Result<(), StreamError> {
        self.ensure_ready()?;
        self.select_mode(policy)?;
        let result = (|| {
            for sample in input {
                if let Some(policy) = policy {
                    let next_count = self
                        .stats
                        .input_frames
                        .checked_add(1)
                        .ok_or(StreamError::InputFrameOverflow)?;
                    self.output_limit = Some(output_frames_for_input(
                        next_count,
                        self.clock.ratio(),
                        policy,
                    )?);
                    if self.timeline_frames != 0 {
                        self.emit_ready(self.timeline_frames - 1, output, &mut evaluate)?;
                    }
                }
                self.ingest(sample.clone(), true, output, &mut evaluate)?;
            }
            Ok(())
        })();
        if result.is_err() {
            self.poison();
        }
        result
    }

    pub(crate) fn finish<T>(
        mut self,
        target: Option<u64>,
        output: &mut Vec<T>,
        mut evaluate: impl FnMut(u64, &[S]) -> Result<ArithmeticOutcome<T>, PolyphaseError>,
    ) -> Result<StreamStats, StreamError> {
        self.ensure_ready()?;
        if let Some(target) = target {
            if let Some(policy) = self.finite_policy {
                let expected =
                    output_frames_for_input(self.stats.input_frames, self.clock.ratio(), policy)?;
                if target != expected {
                    return Err(StreamError::FiniteTargetMismatch {
                        expected,
                        actual: target,
                    });
                }
            }
            if target < self.stats.output_frames {
                return Err(StreamError::TargetBeforeProduced {
                    target,
                    produced: self.stats.output_frames,
                });
            }
            self.output_limit = Some(target);
            if self.timeline_frames != 0 {
                self.emit_ready(self.timeline_frames - 1, output, &mut evaluate)?;
            }
            while self.stats.output_frames < target {
                self.ingest(self.zero.clone(), false, output, &mut evaluate)?;
            }
        } else {
            self.select_mode(None)?;
            if self.stats.input_frames != 0 {
                for _ in 1..self.taps {
                    self.ingest(self.zero.clone(), false, output, &mut evaluate)?;
                }
            }
        }
        Ok(self.stats)
    }

    pub(crate) fn select_mode(
        &mut self,
        policy: Option<FrameCountPolicy>,
    ) -> Result<(), StreamError> {
        match (self.finite_policy, policy) {
            (None, None) => Ok(()),
            (Some(_), None) => Err(StreamError::FiniteAndUnboundedModeMismatch),
            (Some(old), Some(new)) if old == new => Ok(()),
            (Some(_), Some(_)) => Err(StreamError::FinitePolicyMismatch),
            (None, Some(policy)) if self.stats.input_frames == 0 => {
                self.finite_policy = Some(policy);
                Ok(())
            }
            (None, Some(_)) => Err(StreamError::FiniteModeStartedLate),
        }
    }

    fn ingest<T>(
        &mut self,
        sample: S,
        real_input: bool,
        output: &mut Vec<T>,
        evaluate: &mut impl FnMut(u64, &[S]) -> Result<ArithmeticOutcome<T>, PolyphaseError>,
    ) -> Result<(), StreamError> {
        let current_index = self.timeline_frames;
        self.timeline_frames = self
            .timeline_frames
            .checked_add(1)
            .ok_or(StreamError::InputFrameOverflow)?;
        if real_input {
            self.stats.input_frames = self
                .stats
                .input_frames
                .checked_add(1)
                .ok_or(StreamError::InputFrameOverflow)?;
        }
        self.history
            .try_reserve(1)
            .map_err(|_| StreamError::AllocationFailed)?;
        self.history.push_back(sample);
        self.emit_ready(current_index, output, evaluate)
    }

    fn emit_ready<T>(
        &mut self,
        current_index: u64,
        output: &mut Vec<T>,
        evaluate: &mut impl FnMut(u64, &[S]) -> Result<ArithmeticOutcome<T>, PolyphaseError>,
    ) -> Result<(), StreamError> {
        loop {
            if self
                .output_limit
                .is_some_and(|limit| self.stats.output_frames >= limit)
            {
                break;
            }
            let position = self.clock.position();
            if position.input_index > current_index {
                break;
            }
            self.prepare_scratch(position.input_index)?;
            let outcome = evaluate(position.phase, &self.scratch)?;
            let output_frames = self
                .stats
                .output_frames
                .checked_add(1)
                .ok_or(StreamError::OutputFrameOverflow)?;
            let saturated_outputs = self
                .stats
                .saturated_outputs
                .checked_add(u64::from(outcome.saturated))
                .ok_or(StreamError::SaturationCountOverflow)?;
            output
                .try_reserve(1)
                .map_err(|_| StreamError::AllocationFailed)?;
            self.clock.next_position()?;
            output.push(outcome.value);
            self.stats.output_frames = output_frames;
            self.stats.saturated_outputs = saturated_outputs;
        }
        self.prune_history()
    }

    fn prepare_scratch(&mut self, anchor: u64) -> Result<(), StreamError> {
        self.scratch.clear();
        for tap in 0..self.taps {
            let Some(index) = anchor.checked_sub(tap as u64) else {
                self.scratch.push(self.zero.clone());
                continue;
            };
            let unavailable = || StreamError::HistoryUnavailable {
                requested: index,
                oldest: self.history_start_index,
            };
            let offset = index
                .checked_sub(self.history_start_index)
                .ok_or_else(unavailable)?;
            let offset = usize::try_from(offset).map_err(|_| StreamError::InputFrameOverflow)?;
            self.scratch
                .push(self.history.get(offset).ok_or_else(unavailable)?.clone());
        }
        Ok(())
    }

    fn prune_history(&mut self) -> Result<(), StreamError> {
        let retained_taps =
            u64::try_from(self.taps - 1).map_err(|_| StreamError::InputFrameOverflow)?;
        let keep_from = self
            .clock
            .position()
            .input_index
            .saturating_sub(retained_taps);
        while self.history_start_index < keep_from && !self.history.is_empty() {
            self.history.pop_front();
            self.history_start_index = self
                .history_start_index
                .checked_add(1)
                .ok_or(StreamError::InputFrameOverflow)?;
        }
        Ok(())
    }
}
