//! Anki-style scheduling. Pure functions over `Card`, so they are easy to test.

use crate::db::Card;

pub const NEW: i64 = 0;
pub const LEARNING: i64 = 1;
pub const REVIEW: i64 = 2;
pub const RELEARNING: i64 = 3;

pub const AGAIN: i64 = 1;
pub const HARD: i64 = 2;
pub const GOOD: i64 = 3;
pub const EASY: i64 = 4;

pub const DAY: i64 = 86400;
const LEARN_STEPS: &[i64] = &[60, 600];
const RELEARN_STEPS: &[i64] = &[600];
const GRADUATE_DAYS: i64 = 1;
const EASY_DAYS: i64 = 4;
pub const START_EASE: i64 = 2500;
const MIN_EASE: i64 = 1300;
const HARD_FACTOR: f64 = 1.2;
const EASY_BONUS: f64 = 1.3;

fn hard_delay(steps: &[i64], step: i64) -> i64 {
    if steps.len() == 1 {
        return (steps[0] as f64 * 1.5) as i64;
    }
    if step == 0 {
        (steps[0] + steps[1]) / 2
    } else {
        steps[step as usize]
    }
}

fn graduate(c: &mut Card, now: i64, days: i64) {
    c.state = REVIEW;
    c.step = 0;
    c.interval = days;
    c.due = now + days * DAY;
    if c.ease == 0 {
        c.ease = START_EASE;
    }
}

/// Python's `round()` and Rust's `f64::round` disagree on halves: Python rounds them to even.
fn round_days(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// The card's fields after answering `rating` at time `now`.
pub fn schedule(card: &Card, rating: i64, now: i64) -> Card {
    let mut c = card.clone();
    c.reps += 1;

    if matches!(c.state, NEW | LEARNING | RELEARNING) {
        let relearning = c.state == RELEARNING;
        let steps = if relearning {
            RELEARN_STEPS
        } else {
            LEARN_STEPS
        };
        let (state, step) = if c.state == NEW {
            (LEARNING, 0)
        } else {
            (c.state, c.step)
        };
        match rating {
            AGAIN => {
                c.state = state;
                c.step = 0;
                c.due = now + steps[0];
            }
            HARD => {
                c.state = state;
                c.step = step;
                c.due = now + hard_delay(steps, step);
            }
            GOOD => {
                if step + 1 < steps.len() as i64 {
                    c.state = state;
                    c.step = step + 1;
                    c.due = now + steps[(step + 1) as usize];
                } else {
                    let days = if relearning {
                        c.interval.max(GRADUATE_DAYS)
                    } else {
                        GRADUATE_DAYS
                    };
                    graduate(&mut c, now, days);
                }
            }
            _ => {
                let days = if relearning {
                    (c.interval + 1).max(EASY_DAYS)
                } else {
                    EASY_DAYS
                };
                graduate(&mut c, now, days);
            }
        }
        return c;
    }

    let ivl = c.interval.max(1);
    let ease = c.ease;
    if rating == AGAIN {
        c.state = RELEARNING;
        c.step = 0;
        c.due = now + RELEARN_STEPS[0];
        c.lapses += 1;
        c.ease = MIN_EASE.max(ease - 200);
        c.interval = (ivl / 2).max(1);
        return c;
    }
    // Keep the three passing intervals strictly increasing, as Anki does.
    let base = (ivl * ease) as f64 / 1000.0;
    let hard = round_days(ivl as f64 * HARD_FACTOR).max(ivl + 1);
    let good = round_days(base).max(hard + 1);
    let easy = round_days(base * EASY_BONUS).max(good + 1);
    let (days, new_ease) = match rating {
        HARD => (hard, MIN_EASE.max(ease - 150)),
        GOOD => (good, ease),
        _ => (easy, ease + 150),
    };
    c.state = REVIEW;
    c.step = 0;
    c.interval = days;
    c.ease = new_ease;
    c.due = now + days * DAY;
    c
}

/// Short label for a delay, like the buttons show: `<1m`, `10m`, `2h`, `4d`, `1.5mo`.
pub fn fmt_interval(seconds: i64) -> String {
    let s = seconds.max(0) as f64;
    if s < 60.0 {
        "<1m".into()
    } else if s < 3600.0 {
        format!("{}m", (s / 60.0).round_ties_even() as i64)
    } else if s < DAY as f64 {
        format!("{}h", (s / 3600.0).round_ties_even() as i64)
    } else {
        let d = s / DAY as f64;
        if d < 30.0 {
            format!("{}d", d.round_ties_even() as i64)
        } else if d < 365.0 {
            format!("{:.1}mo", d / 30.0)
        } else {
            format!("{:.1}y", d / 365.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Card {
        Card {
            id: 1,
            ease: START_EASE,
            ..Card::default()
        }
    }

    fn review(interval: i64, ease: i64) -> Card {
        Card {
            state: REVIEW,
            interval,
            ease,
            ..fresh()
        }
    }

    const T0: i64 = 1_800_000_000;

    #[test]
    fn new_card_again_stays_in_learning_for_one_minute() {
        let c = schedule(&fresh(), AGAIN, T0);
        assert_eq!((c.state, c.step, c.due), (LEARNING, 0, T0 + 60));
    }

    #[test]
    fn good_walks_learning_steps_then_graduates_to_one_day() {
        let c = schedule(&fresh(), GOOD, T0);
        assert_eq!((c.state, c.step, c.due), (LEARNING, 1, T0 + 600));
        let c = schedule(&c, GOOD, T0 + 600);
        assert_eq!((c.state, c.interval, c.due), (REVIEW, 1, T0 + 600 + DAY));
    }

    #[test]
    fn easy_graduates_new_card_at_four_days() {
        let c = schedule(&fresh(), EASY, T0);
        assert_eq!((c.state, c.interval), (REVIEW, 4));
    }

    #[test]
    fn passing_intervals_strictly_increase_and_ties_round_to_even() {
        // 10 * 2.5 * 1.3 = 32.5: Python rounds this to 32, so the Rust port must too.
        let days: Vec<i64> = [HARD, GOOD, EASY]
            .iter()
            .map(|&r| schedule(&review(10, 2500), r, T0).interval)
            .collect();
        assert_eq!(days, vec![12, 25, 32]);
    }

    #[test]
    fn review_fail_is_a_lapse_that_lowers_ease_and_relearns() {
        let c = schedule(&review(10, 2500), AGAIN, T0);
        assert_eq!(
            (c.state, c.lapses, c.ease, c.due),
            (RELEARNING, 1, 2300, T0 + 600)
        );
        let c = schedule(&c, GOOD, T0 + 600);
        assert_eq!((c.state, c.interval), (REVIEW, 5));
    }

    #[test]
    fn ease_never_drops_below_floor() {
        assert_eq!(schedule(&review(3, 1350), AGAIN, T0).ease, MIN_EASE);
    }

    #[test]
    fn hard_in_learning_waits_between_the_first_two_steps() {
        assert_eq!(schedule(&fresh(), HARD, T0).due, T0 + 330);
    }

    #[test]
    fn interval_labels() {
        let labels: Vec<String> = [30, 600, 7200, 4 * DAY, 45 * DAY]
            .iter()
            .map(|&s| fmt_interval(s))
            .collect();
        assert_eq!(labels, vec!["<1m", "10m", "2h", "4d", "1.5mo"]);
    }
}
