/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! What each adaptive trigger does under each profile.

use crate::config::{Games, Params, TriggerProfile};
use crate::games::{self, Kind};
use crate::protocol::TriggerEffect;

/// One trigger's behaviour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Side {
    /// Effect while nothing is happening.
    pub rest: TriggerEffect,
    /// Kick strength on an impact, 0 (none) to 8.
    pub kick: u8,
}

/// Behaviour of both triggers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    pub right: Side,
    pub left: Side,
}

const FREE: Side = Side { rest: TriggerEffect::OFF, kick: 0 };

/// `Auto` resolved against the running game; other profiles pass through.
pub fn resolve(profile: TriggerProfile, appid: Option<u32>, extra: &Games) -> TriggerProfile {
    match profile {
        TriggerProfile::Auto => match appid.and_then(|id| games::classify(id, extra)) {
            Some(Kind::Shooter) => TriggerProfile::Shooter,
            Some(Kind::Racing) => TriggerProfile::Racing,
            // Unknown game, or none: follow the haptics on both triggers.
            None => TriggerProfile::Generic,
        },
        other => other,
    }
}

/// The plan for a resolved profile. `None` means the triggers are left alone.
pub fn plan(profile: TriggerProfile, p: &Params) -> Option<Plan> {
    let resistance = p.trigger_resistance as u8; // 0..8
    let kick = p.trigger_strength as u8; // 0..8
    let steady = |level: u8| if level == 0 { TriggerEffect::OFF } else { TriggerEffect::feedback(1, level) };
    match profile {
        TriggerProfile::Off | TriggerProfile::Auto => None,
        // Gun: a click point on R2 and the full kick; L2 (aim) stays free.
        TriggerProfile::Shooter => Some(Plan {
            right: Side {
                rest: if resistance == 0 { TriggerEffect::OFF } else { TriggerEffect::weapon(2, 5, resistance) },
                kick,
            },
            left: FREE,
        }),
        // Pedals: a firm brake on L2 with half the kick, a light throttle on R2
        // with the faintest kick.
        TriggerProfile::Racing => Some(Plan {
            right: Side { rest: steady(resistance.div_ceil(3)), kick: kick.min(1) },
            left: Side {
                rest: steady(if resistance == 0 { 0 } else { (resistance + 2).min(8) }),
                kick: kick.div_ceil(2),
            },
        }),
        // Anything else: both triggers kick with the haptics, no resistance.
        TriggerProfile::Generic => {
            let side = Side { rest: TriggerEffect::OFF, kick };
            Some(Plan { right: side, left: side })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Preset;

    fn params(resistance: f32, strength: f32) -> Params {
        Params { trigger_resistance: resistance, trigger_strength: strength, ..Preset::Game.params() }
    }

    #[test]
    fn auto_follows_the_game() {
        let none = Games::default();
        assert_eq!(resolve(TriggerProfile::Auto, Some(730), &none), TriggerProfile::Shooter);
        assert_eq!(resolve(TriggerProfile::Auto, Some(1551360), &none), TriggerProfile::Racing);
        assert_eq!(resolve(TriggerProfile::Auto, Some(42), &none), TriggerProfile::Generic);
        assert_eq!(resolve(TriggerProfile::Auto, None, &none), TriggerProfile::Generic);
        // A fixed choice ignores the game.
        assert_eq!(resolve(TriggerProfile::Racing, Some(730), &none), TriggerProfile::Racing);
        assert_eq!(resolve(TriggerProfile::Off, Some(730), &none), TriggerProfile::Off);
    }

    #[test]
    fn off_leaves_triggers_alone() {
        assert_eq!(plan(TriggerProfile::Off, &params(5.0, 8.0)), None);
    }

    #[test]
    fn shooter_is_gun_on_r2_only() {
        let p = plan(TriggerProfile::Shooter, &params(5.0, 8.0)).unwrap();
        assert_eq!(p.right.rest, TriggerEffect::weapon(2, 5, 5));
        assert_eq!(p.right.kick, 8);
        assert_eq!(p.left, FREE);
    }

    #[test]
    fn racing_brake_is_firmer_than_throttle() {
        let p = plan(TriggerProfile::Racing, &params(5.0, 8.0)).unwrap();
        assert_eq!(p.left.rest, TriggerEffect::feedback(1, 7));
        assert_eq!(p.right.rest, TriggerEffect::feedback(1, 2));
        assert_eq!((p.left.kick, p.right.kick), (4, 1));
        // No resistance and no kick requested: nothing on either side.
        let none = plan(TriggerProfile::Racing, &params(0.0, 0.0)).unwrap();
        assert_eq!((none.left, none.right), (FREE, FREE));
    }

    #[test]
    fn generic_kicks_both_without_resistance() {
        let p = plan(TriggerProfile::Generic, &params(5.0, 6.0)).unwrap();
        assert_eq!(p.right, Side { rest: TriggerEffect::OFF, kick: 6 });
        assert_eq!(p.left, p.right);
    }
}
