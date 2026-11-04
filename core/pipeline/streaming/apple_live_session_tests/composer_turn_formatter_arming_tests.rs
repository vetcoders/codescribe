/// rc-w2-composer-turn: the live paid-formatter arming seam.
///
/// Deliberately its own module. The file's main `mod tests` carries a large
/// parked surface; a falsifier for a new production owner must not inherit that
/// state, and must not be silently disabled with it.
    use super::{CaptureTurnIntent, FormattingPolicy, live_formatter_lane_is_armed};

    /// Every configuration that would arm the live lane for a hands-free take.
    const FULLY_ARMED: (FormattingPolicy, bool) = (FormattingPolicy::Correction, true);

    #[test]
    fn unarmed_live_formatting_never_resolves_credentials() {
        for (intent, policy) in [
            (
                CaptureTurnIntent::AgentChannel,
                FormattingPolicy::Correction,
            ),
            (CaptureTurnIntent::AgentChannel, FormattingPolicy::Smart),
            (CaptureTurnIntent::AgentChannel, FormattingPolicy::Max),
            (CaptureTurnIntent::SingleTurn, FormattingPolicy::Correction),
            (CaptureTurnIntent::HandsFree, FormattingPolicy::Off),
            (CaptureTurnIntent::HandsFree, FormattingPolicy::Max),
            (CaptureTurnIntent::SingleTurn, FormattingPolicy::Max),
        ] {
            assert!(!live_formatter_lane_is_armed(intent, policy, || {
                panic!("an unarmed live lane must not enter the credential-use boundary")
            }));
        }
    }

    #[test]
    fn a_one_turn_take_never_arms_the_live_formatter_lane() {
        let (policy, available) = FULLY_ARMED;
        assert!(
            !live_formatter_lane_is_armed(CaptureTurnIntent::SingleTurn, policy, || available),
            "a composer turn must not open paid provider slots per sealed fragment, \
             even with a non-Off formatting level and a live lane"
        );
    }

    #[test]
    fn a_hands_free_take_keeps_its_existing_live_lane() {
        let (policy, available) = FULLY_ARMED;
        assert!(
            live_formatter_lane_is_armed(CaptureTurnIntent::HandsFree, policy, || available),
            "hotkey, tray and overlay takes keep the per-occurrence formatter they had"
        );
    }

    /// The intent is an additional refusal, never a way to arm a lane the
    /// configuration itself refuses.
    #[test]
    fn hands_free_cannot_arm_a_lane_the_configuration_refuses() {
        for (policy, available) in [
            (FormattingPolicy::Off, true),
            (FormattingPolicy::Correction, false),
        ] {
            assert!(
                !live_formatter_lane_is_armed(CaptureTurnIntent::HandsFree, policy, || available),
                "capture intent must not override formatter configuration or transport ownership"
            );
        }
    }
