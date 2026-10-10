//! CR 602.2b + CR 601.2g-h: an activated ability is paid the way a spell is.
//! The player activates mana abilities while paying, so an activation in
//! `CastPaymentMode::Manual` stops in `ManaPayment` once its X and targets
//! are announced, exactly as a manual cast does, and nothing is tapped for the
//! player. Cancelling there reverses the whole activation (CR 733.1).
//!
//! https://github.com/phase-rs/phase/issues/6016

use crate::support::shared_card_db;
use engine::ai_support::legal_actions_full;
use engine::game::printed_cards::rehydrate_game_from_card_db;
use engine::game::scenario::{GameRunner, GameScenario, P0};
use engine::game::scenario_db::GameScenarioDbExt;
use engine::types::ability::TargetRef;
use engine::types::actions::GameAction;
use engine::types::game_state::{CastPaymentMode, WaitingFor};
use engine::types::identifiers::ObjectId;
use engine::types::phase::Phase;
use engine::types::zones::Zone;

struct EquipBoard {
    runner: GameRunner,
    bonesplitter: ObjectId,
    bears: ObjectId,
    sol_ring: ObjectId,
    mountain: ObjectId,
}

/// Bonesplitter (Equip {1}) and a creature to equip, with Sol Ring and a
/// Mountain untapped: either source could pay the {1}.
fn equip_board() -> EquipBoard {
    let db = shared_card_db().expect("the checked-in fixture contains the cards");
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let bonesplitter = scenario.add_real_card(P0, "Bonesplitter", Zone::Battlefield, db);
    let bears = scenario.add_real_card(P0, "Grizzly Bears", Zone::Battlefield, db);
    let sol_ring = scenario.add_real_card(P0, "Sol Ring", Zone::Battlefield, db);
    let mountain = scenario.add_real_card(P0, "Mountain", Zone::Battlefield, db);
    let mut runner = scenario.build();
    rehydrate_game_from_card_db(runner.state_mut(), db);
    EquipBoard {
        runner,
        bonesplitter,
        bears,
        sol_ring,
        mountain,
    }
}

/// The index of the only non-mana activated ability `source` offers now.
fn activated_ability_index(runner: &GameRunner, source: ObjectId) -> usize {
    let (_, _, grouped) = legal_actions_full(runner.state());
    grouped
        .get(&source)
        .into_iter()
        .flatten()
        .find_map(|action| match action {
            GameAction::ActivateAbility { ability_index, .. } => Some(*ability_index),
            _ => None,
        })
        .expect("the source's activated ability is offered")
}

fn activate(
    runner: &mut GameRunner,
    source_id: ObjectId,
    payment_mode: CastPaymentMode,
) -> WaitingFor {
    let ability_index = activated_ability_index(runner, source_id);
    runner
        .act(GameAction::ActivateAbility {
            source_id,
            ability_index,
            payment_mode,
        })
        .expect("the activation begins")
        .waiting_for
}

/// Answer a target prompt with `targets`, if the engine asks one.
fn choose_targets(runner: &mut GameRunner, targets: &[ObjectId]) -> WaitingFor {
    if !matches!(
        runner.state().waiting_for,
        WaitingFor::TargetSelection { .. }
    ) {
        return runner.state().waiting_for.clone();
    }
    runner
        .act(GameAction::SelectTargets {
            targets: targets.iter().copied().map(TargetRef::Object).collect(),
        })
        .expect("the targets are legal")
        .waiting_for
}

fn tap_land_for_mana(runner: &mut GameRunner, land: ObjectId) {
    let (_, _, grouped) = legal_actions_full(runner.state());
    let selection = grouped
        .get(&land)
        .into_iter()
        .flatten()
        .find_map(|action| match action {
            GameAction::TapLandForMana { selection } => Some(selection.clone()),
            _ => None,
        })
        .expect("the engine offers the land's mana ability while paying");
    runner
        .act(GameAction::TapLandForMana { selection })
        .expect("tap the land");
}

fn tapped(runner: &GameRunner, id: ObjectId) -> bool {
    runner.state().objects[&id].tapped
}

/// CR 602.2b + CR 601.2g: Manual Equip {1} asks how to pay after the target is
/// chosen; the player taps the Mountain and Sol Ring is never touched.
#[test]
fn a_manual_equip_waits_for_the_player_to_tap_the_land_and_leaves_sol_ring_untapped() {
    let EquipBoard {
        mut runner,
        bonesplitter,
        bears,
        sol_ring,
        mountain,
    } = equip_board();

    activate(&mut runner, bonesplitter, CastPaymentMode::Manual);
    let waiting_for = choose_targets(&mut runner, &[bears]);
    assert!(
        matches!(waiting_for, WaitingFor::ManaPayment { player, .. } if player == P0),
        "CR 601.2g: a manual activation stops for its mana payment, got {waiting_for:?}"
    );
    assert!(
        !tapped(&runner, sol_ring),
        "nothing is tapped for the player"
    );
    assert!(
        !tapped(&runner, mountain),
        "nothing is tapped for the player"
    );
    assert!(
        runner.state().stack.is_empty(),
        "nothing is on the stack yet"
    );

    tap_land_for_mana(&mut runner, mountain);
    let waiting_for = runner
        .act(GameAction::PassPriority)
        .expect("the paid activation completes")
        .waiting_for;
    assert!(
        matches!(waiting_for, WaitingFor::Priority { .. }),
        "{waiting_for:?}"
    );
    assert_eq!(
        runner.state().stack.len(),
        1,
        "the equip ability is on the stack"
    );
    assert!(tapped(&runner, mountain), "the player's chosen land paid");
    assert!(!tapped(&runner, sol_ring), "Sol Ring was not used to pay");

    runner.advance_until_stack_empty();
    assert!(
        runner.state().objects[&bears]
            .attachments
            .contains(&bonesplitter),
        "the equip ability resolves onto the chosen creature"
    );
}

/// The default stays automatic: an Auto Equip {1} is paid without a
/// `ManaPayment` window and reaches the stack straight from its target.
#[test]
fn an_auto_equip_still_pays_automatically() {
    let EquipBoard {
        mut runner,
        bonesplitter,
        bears,
        sol_ring,
        mountain,
    } = equip_board();

    activate(&mut runner, bonesplitter, CastPaymentMode::Auto);
    let waiting_for = choose_targets(&mut runner, &[bears]);
    assert!(
        matches!(waiting_for, WaitingFor::Priority { .. }),
        "an automatic activation needs no payment prompt, got {waiting_for:?}"
    );
    assert_eq!(
        runner.state().stack.len(),
        1,
        "the equip ability is on the stack"
    );
    assert!(
        tapped(&runner, sol_ring) || tapped(&runner, mountain),
        "the engine tapped a source to pay {{1}}"
    );
}

/// CR 601.2b + CR 601.2c + CR 601.2g: Candelabra of Tawnos announces X, then
/// its X targets, then is paid. Paid manually, the two target lands tap for
/// the {2} and the ability untaps them; Sol Ring, which alone could pay {2},
/// stays untapped.
#[test]
fn a_manual_x_activation_is_paid_with_the_mana_abilities_the_player_chooses() {
    let db = shared_card_db().expect("the checked-in fixture contains the cards");
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let candelabra = scenario.add_real_card(P0, "Candelabra of Tawnos", Zone::Battlefield, db);
    let island_a = scenario.add_real_card(P0, "Island", Zone::Battlefield, db);
    let island_b = scenario.add_real_card(P0, "Island", Zone::Battlefield, db);
    let sol_ring = scenario.add_real_card(P0, "Sol Ring", Zone::Battlefield, db);
    let mut runner = scenario.build();
    rehydrate_game_from_card_db(runner.state_mut(), db);

    let waiting_for = activate(&mut runner, candelabra, CastPaymentMode::Manual);
    assert!(
        matches!(waiting_for, WaitingFor::ChooseXValue { .. }),
        "CR 601.2b: X is announced first, got {waiting_for:?}"
    );
    runner
        .act(GameAction::ChooseX { value: 2 })
        .expect("X = 2 is affordable");
    let waiting_for = choose_targets(&mut runner, &[island_a, island_b]);
    assert!(
        matches!(waiting_for, WaitingFor::ManaPayment { .. }),
        "CR 601.2g: the targets are chosen, then the cost is paid, got {waiting_for:?}"
    );
    assert!(
        !tapped(&runner, sol_ring),
        "nothing is tapped for the player"
    );

    tap_land_for_mana(&mut runner, island_a);
    tap_land_for_mana(&mut runner, island_b);
    runner
        .act(GameAction::PassPriority)
        .expect("the paid activation completes");
    assert_eq!(runner.state().stack.len(), 1, "the ability is on the stack");
    assert!(!tapped(&runner, sol_ring), "Sol Ring was not used to pay");

    runner.advance_until_stack_empty();
    assert!(
        !tapped(&runner, island_a) && !tapped(&runner, island_b),
        "the ability untaps the lands that paid for it"
    );
    assert!(tapped(&runner, candelabra), "{{T}} was paid");
}

/// CR 733.1 + CR 733.2: cancelling an activation during its manual payment
/// reverses the whole activation, exactly as cancelling a manual cast does:
/// nothing is on the stack, nothing is attached and the activation is not
/// counted. The mana ability activated while paying is the player's to keep
/// (CR 733.1 lets them reverse it, it does not make them), so its mana floats
/// and pays for the activation when the player redoes it.
#[test]
fn cancelling_a_manual_activation_during_payment_reverses_it() {
    let EquipBoard {
        mut runner,
        bonesplitter,
        bears,
        sol_ring,
        mountain,
    } = equip_board();

    activate(&mut runner, bonesplitter, CastPaymentMode::Manual);
    let waiting_for = choose_targets(&mut runner, &[bears]);
    assert!(
        matches!(waiting_for, WaitingFor::ManaPayment { .. }),
        "{waiting_for:?}"
    );
    tap_land_for_mana(&mut runner, mountain);

    let waiting_for = runner
        .act(GameAction::CancelCast)
        .expect("the activation can be cancelled while paying")
        .waiting_for;
    assert!(
        matches!(waiting_for, WaitingFor::Priority { player } if player == P0),
        "CR 733.2: the player who had priority retains it, got {waiting_for:?}"
    );
    assert!(runner.state().stack.is_empty(), "the activation is gone");
    assert!(runner.state().pending_cast.is_none());
    assert!(
        runner.state().objects[&bears].attachments.is_empty(),
        "nothing was equipped"
    );
    assert!(
        !runner
            .state()
            .activated_abilities_this_turn
            .keys()
            .any(|key| key.0 == bonesplitter),
        "a reversed activation is not counted"
    );
    assert!(!tapped(&runner, sol_ring), "Sol Ring was never touched");
    assert_eq!(
        runner.state().players[P0.0 as usize].mana_pool.total(),
        1,
        "the Mountain's mana floats, as after a cancelled manual cast"
    );

    // CR 733.2: the player may redo the reversed action legally; the floating
    // mana pays for it.
    activate(&mut runner, bonesplitter, CastPaymentMode::Manual);
    choose_targets(&mut runner, &[bears]);
    runner
        .act(GameAction::PassPriority)
        .expect("the floating mana pays the redone activation");
    assert_eq!(runner.state().stack.len(), 1);
    assert!(!tapped(&runner, sol_ring), "Sol Ring was never touched");
}

/// Bonesplitter's equip aimed at Heartfire Hero (Valiant: the first time each
/// turn it becomes the target of a spell or ability its controller controls,
/// it gets a +1/+1 counter), paid manually.
fn valiant_board() -> (GameRunner, ObjectId, ObjectId, ObjectId) {
    let db = shared_card_db().expect("the checked-in fixture contains the cards");
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let bonesplitter = scenario.add_real_card(P0, "Bonesplitter", Zone::Battlefield, db);
    let hero = scenario.add_real_card(P0, "Heartfire Hero", Zone::Battlefield, db);
    let mountain = scenario.add_real_card(P0, "Mountain", Zone::Battlefield, db);
    let mut runner = scenario.build();
    rehydrate_game_from_card_db(runner.state_mut(), db);
    activate(&mut runner, bonesplitter, CastPaymentMode::Manual);
    let waiting_for = choose_targets(&mut runner, &[hero]);
    assert!(
        matches!(waiting_for, WaitingFor::ManaPayment { .. }),
        "{waiting_for:?}"
    );
    assert!(
        runner.state().stack.is_empty(),
        "CR 603.3: the targeting trigger waits for the activation to finish"
    );
    tap_land_for_mana(&mut runner, mountain);
    (runner, bonesplitter, hero, mountain)
}

fn plus_one_counters(runner: &GameRunner, id: ObjectId) -> u32 {
    runner.state().objects[&id]
        .counters
        .get(&engine::types::counter::CounterType::Plus1Plus1)
        .copied()
        .unwrap_or(0)
}

/// CR 602.2b + CR 603.2 + CR 603.3: the trigger the activation's target caused
/// is collected across the manual payment window and put on the stack above the
/// ability once it is activated.
#[test]
fn a_targeting_trigger_survives_the_manual_payment_window() {
    let (mut runner, bonesplitter, hero, _) = valiant_board();
    runner
        .act(GameAction::PassPriority)
        .expect("the paid activation completes");
    assert_eq!(
        runner.state().stack.len(),
        2,
        "the equip ability and the valiant trigger above it"
    );
    runner.advance_until_stack_empty();
    assert_eq!(plus_one_counters(&runner, hero), 1, "valiant resolved once");
    assert!(runner.state().objects[&hero]
        .attachments
        .contains(&bonesplitter));
}

/// CR 733.1: no abilities trigger as a result of an undone action, so a
/// cancelled manual activation leaves its targeting trigger behind with it.
#[test]
fn a_cancelled_manual_activation_triggers_nothing() {
    let (mut runner, _, hero, _) = valiant_board();
    runner
        .act(GameAction::CancelCast)
        .expect("the activation can be cancelled while paying");
    assert!(
        runner.state().stack.is_empty(),
        "no trigger was put on the stack"
    );
    runner.advance_until_stack_empty();
    assert_eq!(plus_one_counters(&runner, hero), 0);
}

/// Marauding Mako's discard trigger (CR 702.29a: cycling discards the card as
/// a cost) and an instant with Cycling {1} in hand.
const DISCARD_GROWER: &str =
    "Whenever you discard one or more cards, put that many +1/+1 counters on this creature.";
const CYCLER: &str = "Draw a card.\nCycling {1}";

/// CR 602.2b + CR 603.2 + CR 603.3: an untargeted activation whose cost
/// discards (cycling) holds the triggers that discard causes across the manual
/// payment window and puts them on the stack once it is activated.
#[test]
fn a_cost_trigger_of_an_untargeted_activation_survives_the_manual_payment_window() {
    let db = shared_card_db().expect("the checked-in fixture contains the cards");
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let grower = scenario
        .add_creature_from_oracle(P0, "Discard Grower", 2, 2, DISCARD_GROWER)
        .id();
    let cycler = scenario
        .add_spell_to_hand_from_oracle(P0, "Cycler", true, CYCLER)
        .id();
    scenario.add_card_to_library_top(P0, "Library Card");
    let mountain = scenario.add_real_card(P0, "Mountain", Zone::Battlefield, db);
    let mut runner = scenario.build();

    let waiting_for = activate(&mut runner, cycler, CastPaymentMode::Manual);
    assert!(
        matches!(waiting_for, WaitingFor::ManaPayment { .. }),
        "{waiting_for:?}"
    );
    tap_land_for_mana(&mut runner, mountain);
    runner
        .act(GameAction::PassPriority)
        .expect("the paid activation completes");
    assert_eq!(
        runner.state().stack.len(),
        2,
        "the cycling ability and the discard trigger above it"
    );
    runner.advance_until_stack_empty();
    assert_eq!(plus_one_counters(&runner, grower), 1);
}
