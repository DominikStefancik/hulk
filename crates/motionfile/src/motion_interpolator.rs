use std::fmt::Debug;
use std::time::Duration;

use crate::condition::{ContinuousConditionType, DiscreteConditionType, Response, TimeOut};
use crate::timed_spline::{InterpolatorError, TimedSpline};
use crate::Condition;
use crate::MotionFile;
use color_eyre::{Report, Result};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use splines::Interpolate;
use types::ConditionInput;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ConditionedSpline<T> {
    pub entry_condition: Option<DiscreteConditionType>,
    pub motion_interrupts: Option<Vec<ContinuousConditionType>>,
    pub spline: TimedSpline<T>,
    pub exit_condition: Option<DiscreteConditionType>,
}

#[derive(Default, Debug)]
pub struct MotionInterpolator<T> {
    frames: Vec<ConditionedSpline<T>>,
    current_state: State<T>,
}

#[derive(Debug, Clone, Copy)]
enum State<T> {
    CheckEntry {
        current_frame_index: usize,
        time_since_start: Duration,
    },
    InterpolateSpline {
        current_frame_index: usize,
        time_since_start: Duration,
    },
    CheckExit {
        current_frame_index: usize,
        time_since_start: Duration,
    },
    Finished,
    Aborted {
        at_position: T,
    },
}

impl<T> State<T> {
    pub fn current_frame_index(&self) -> Option<usize> {
        match self {
            State::CheckEntry {
                current_frame_index,
                ..
            }
            | State::InterpolateSpline {
                current_frame_index,
                ..
            }
            | State::CheckExit {
                current_frame_index,
                ..
            } => Some(*current_frame_index),
            _ => None,
        }
    }
}

enum ReturnState {
    Return,
    Continue,
}

impl<T> Default for State<T> {
    fn default() -> Self {
        State::CheckEntry {
            current_frame_index: 0,
            time_since_start: Duration::ZERO,
        }
    }
}

impl<T: Debug + Interpolate<f32>> MotionInterpolator<T> {
    fn check_continuous_conditions(&mut self, condition_input: &ConditionInput) -> ReturnState {
        if let Some(continuous_conditions) = self
            .current_state
            .current_frame_index()
            .and_then(|frame_index| self.frames[frame_index].motion_interrupts.as_ref())
        {
            return match continuous_conditions
                .iter()
                .map(|condition| condition.evaluate(condition_input))
                .reduce(|accumulated, current| match (&accumulated, &current) {
                    (Response::Abort, _) => Response::Abort,
                    (_, Response::Abort) => Response::Abort,
                    (Response::Wait, _) => Response::Wait,
                    (_, Response::Wait) => Response::Wait,
                    _ => accumulated,
                }) {
                Some(Response::Abort) => {
                    self.current_state = State::Aborted {
                        at_position: self.value(),
                    };
                    ReturnState::Return
                }
                Some(Response::Wait) => ReturnState::Return,
                _ => ReturnState::Continue,
            };
        }

        ReturnState::Continue
    }

    fn advance_state(&mut self, time_step: Duration, condition_input: &ConditionInput) {
        self.current_state = match self.current_state {
            State::CheckEntry {
                current_frame_index,
                time_since_start,
            } => {
                let current_frame = &self.frames[current_frame_index];
                match current_frame.entry_condition.as_ref().map(|condition| {
                    condition
                        .evaluate(condition_input)
                        .with_timeout(condition.timeout(time_since_start))
                }) {
                    Some(Response::Abort) => State::Aborted {
                        at_position: self.value(),
                    },
                    Some(Response::Wait) => State::CheckEntry {
                        current_frame_index,
                        time_since_start: time_since_start + time_step,
                    },
                    _ => State::InterpolateSpline {
                        current_frame_index,
                        time_since_start: Duration::ZERO,
                    },
                }
            }
            State::InterpolateSpline {
                current_frame_index,
                time_since_start,
            } => {
                let current_frame = &self.frames[current_frame_index];
                if time_since_start >= current_frame.spline.total_duration() {
                    State::CheckExit {
                        current_frame_index,
                        time_since_start: Duration::ZERO,
                    }
                } else {
                    State::InterpolateSpline {
                        current_frame_index,
                        time_since_start: time_since_start + time_step,
                    }
                }
            }
            State::CheckExit {
                current_frame_index,
                time_since_start,
            } => {
                let current_frame = &self.frames[current_frame_index];
                match current_frame.exit_condition.as_ref().map(|condition| {
                    condition
                        .evaluate(condition_input)
                        .with_timeout(condition.timeout(time_since_start))
                }) {
                    Some(Response::Abort) => State::Aborted {
                        at_position: self.value(),
                    },
                    Some(Response::Wait) => State::CheckExit {
                        current_frame_index,
                        time_since_start: time_since_start + time_step,
                    },
                    _ if current_frame_index < self.frames.len() - 1 => State::CheckEntry {
                        current_frame_index: current_frame_index + 1,
                        time_since_start: Duration::ZERO,
                    },
                    _ => State::Finished,
                }
            }
            other_state => other_state,
        };
    }

    pub fn advance_by(&mut self, time_step: Duration, condition_input: &ConditionInput) {
        if let ReturnState::Return = self.check_continuous_conditions(condition_input) {
            return;
        }

        self.advance_state(time_step, condition_input);
    }

    pub fn is_finished(&self) -> bool {
        matches!(self.current_state, State::Finished | State::Aborted { .. })
    }

    pub fn value(&self) -> T {
        match self.current_state {
            State::CheckEntry {
                current_frame_index,
                ..
            } => self.frames[current_frame_index].spline.start_position(),
            State::InterpolateSpline {
                current_frame_index,
                time_since_start,
            } => self.frames[current_frame_index]
                .spline
                .value_at(time_since_start),
            State::CheckExit {
                current_frame_index,
                ..
            } => self.frames[current_frame_index].spline.end_position(),
            State::Finished => self.frames.last().unwrap().spline.end_position(),
            State::Aborted { at_position } => at_position,
        }
    }

    pub fn reset(&mut self) {
        self.current_state = State::CheckEntry {
            current_frame_index: 0,
            time_since_start: Duration::ZERO,
        };
    }

    pub fn set_initial_positions(&mut self, position: T) {
        if let Some(keyframe) = self.frames.first_mut() {
            keyframe.spline.set_initial_positions(position);
        }
    }
}

impl<T: Debug + Interpolate<f32>> TryFrom<MotionFile<T>> for MotionInterpolator<T> {
    type Error = Report;

    fn try_from(motion_file: MotionFile<T>) -> Result<Self> {
        let interpolation_mode = motion_file.interpolation_mode;

        let first_frame = motion_file.motion.first().unwrap();

        let mut motion_frames = vec![ConditionedSpline {
            entry_condition: first_frame.entry_condition.clone(),
            motion_interrupts: first_frame.motion_interrupts.clone(),
            spline: TimedSpline::try_new_with_start(
                motion_file.initial_positions,
                first_frame.keyframes.clone(),
                interpolation_mode,
            )?,
            exit_condition: first_frame.exit_condition.clone(),
        }];

        motion_frames.extend(
            motion_file
                .motion
                .into_iter()
                .tuple_windows()
                .map(|(first_frame, second_frame)| {
                    Ok(ConditionedSpline {
                        entry_condition: second_frame.entry_condition,
                        motion_interrupts: second_frame.motion_interrupts,
                        spline: TimedSpline::try_new_with_start(
                            first_frame.keyframes.last().unwrap().positions,
                            second_frame.keyframes,
                            interpolation_mode,
                        )?,
                        exit_condition: second_frame.exit_condition,
                    })
                })
                .collect::<Result<Vec<_>, InterpolatorError>>()?,
        );

        Ok(Self {
            current_state: State::CheckEntry {
                current_frame_index: 0,
                time_since_start: Duration::ZERO,
            },
            frames: motion_frames,
        })
    }
}
