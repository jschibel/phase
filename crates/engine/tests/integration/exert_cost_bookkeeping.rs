//! Exert paid as part of an activated ability's cost records the exertion, and
//! "whenever you exert a creature" observers see it.
//!
//! Before the fix the `AbilityCost::Exert` arm only installed the "doesn't
//! untap" effect: it never recorded the permanent in `exerted_this_turn` and
//! emitted no `CreatureExerted`. Separately `match_exerted` ignored the
//! trigger's `valid_card` / `valid_target`, so an observer could only ever fire
//! for its own source.
//!
//! Rules: CR 701.43a (exert = choose to have a permanent not untap during your
//! next untap step; an event each time), CR 701.43b (an already-exerted
//! permanent can be exerted again), CR 701.43d (the attack cost and its linked
//! trigger), CR 602.2b + CR 605.3b (exert paid while activating an ability,
//! mana abilities included), CR 601.2h (a mana ability activated while paying a
//! spell).
//!
//! No printed card is both a "whenever you exert" observer and an exert-cost
//! creature, so the observer tests pair Resolute Survivors (observer) with a
//! cost-exerter (Arena of Glory, Fervent Paincaster, Oasis Ritualist).

use engine::game::combat::AttackTarget;
use engine::game::scenario::{CastOutcome, GameRunner, GameScenario, P0, P1};
use engine::types::ability::{AbilityCost, ContinuousModification, TargetFilter};
use engine::types::actions::GameAction;
use engine::types::events::GameEvent;
use engine::types::game_state::{CastPaymentMode, ManaChoice, WaitingFor};
use engine::types::identifiers::ObjectId;
use engine::types::mana::{ManaColor, ManaCost, ManaCostShard, ManaType};
use engine::types::phase::Phase;
use engine::types::player::PlayerId;
use engine::types::statics::StaticMode;
use engine::types::zones::Zone;

const OASIS_RITUALIST: &str = "{T}: Add one mana of any color.\n\
{T}, Exert this creature: Add two mana of any one color. \
(An exerted creature won't untap during your next untap step.)";
const ARENA_OF_GLORY: &str = "This land enters tapped unless you control a Mountain.\n\
{T}: Add {R}.\n\
{R}, {T}, Exert this land: Add {R}{R}. If any of that mana is spent on a creature spell, \
it gains haste until end of turn. (An exerted permanent won't untap during your next untap step.)";
const FERVENT_PAINCASTER: &str = "{T}: This creature deals 1 damage to target player or \
planeswalker.\n{T}, Exert this creature: It deals 1 damage to target creature.";
const RESOLUTE_SURVIVORS: &str = "You may exert this creature as it attacks. (It won't untap \
during your next untap step.)\nWhenever you exert a creature, this creature deals 1 damage to \
each opponent and you gain 1 life.";
const BITTERBLADE_WARRIOR: &str = "You may exert this creature as it attacks. When you do, it \
gets +1/+0 and gains deathtouch until end of turn. (An exerted creature won't untap during your \
next untap step.)";

fn life(runner: &GameRunner, player: PlayerId) -> i32 {
    runner
        .state()
        .players
        .iter()
        .find(|p| p.id == player)
        .expect("player exists")
        .life
}

fn exerted_events(events: &[GameEvent]) -> Vec<(ObjectId, PlayerId)> {
    events
        .iter()
        .filter_map(|event| match event {
            GameEvent::CreatureExerted { object_id, player } => Some((*object_id, *player)),
            _ => None,
        })
        .collect()
}

/// Transient `CantUntap` effects aimed at `id` (the shape an exert installs).
fn cant_untap_markers(runner: &GameRunner, id: ObjectId) -> usize {
    runner
        .state()
        .transient_continuous_effects
        .iter()
        .filter(|e| e.affected == TargetFilter::SpecificObject { id })
        .filter(|e| {
            e.modifications.iter().any(|m| {
                matches!(
                    m,
                    ContinuousModification::AddStaticMode {
                        mode: StaticMode::CantUntap
                    }
                )
            })
        })
        .count()
}

fn pool_units(runner: &GameRunner, color: ManaType) -> usize {
    runner
        .state()
        .players
        .iter()
        .find(|p| p.id == P0)
        .expect("P0 exists")
        .mana_pool
        .units()
        .filter(|unit| unit.color == color)
        .count()
}

/// Index of the ability whose cost is a `Composite` (the `..., Exert` ability),
/// so a test can never activate the plain `{T}:` ability by accident.
fn composite_index(runner: &GameRunner, source: ObjectId) -> usize {
    let abilities = &runner.state().objects[&source].abilities;
    let indices: Vec<usize> = abilities
        .iter()
        .enumerate()
        .filter(|(_, a)| matches!(a.cost, Some(AbilityCost::Composite { .. })))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(indices.len(), 1, "exactly one Composite-cost ability");
    indices[0]
}

/// Index of the ability whose cost is exactly `{T}`.
fn tap_only_index(runner: &GameRunner, source: ObjectId) -> usize {
    runner.state().objects[&source]
        .abilities
        .iter()
        .position(|a| matches!(a.cost, Some(AbilityCost::Tap)))
        .expect("a Tap-only ability")
}

fn untap(runner: &mut GameRunner, ids: &[ObjectId]) {
    for id in ids {
        runner.state_mut().objects.get_mut(id).unwrap().tapped = false;
    }
}

/// Run `action`, appending its events to `acc`.
fn act_collect(runner: &mut GameRunner, action: GameAction, acc: &mut Vec<GameEvent>) {
    let result = runner.act(action).expect("action should succeed");
    acc.extend(result.events);
}

/// Activate Oasis Ritualist's ability `index` and answer the color prompt with
/// Green. (`AbilityActivation::resolve` does not drive `ChooseManaColor`.)
fn activate_ritualist(
    runner: &mut GameRunner,
    ritualist: ObjectId,
    index: usize,
    acc: &mut Vec<GameEvent>,
) {
    act_collect(
        runner,
        GameAction::ActivateAbility {
            source_id: ritualist,
            ability_index: index,
        },
        acc,
    );
    assert_eq!(runner.waiting_for_kind(), "ChooseManaColor");
    act_collect(
        runner,
        GameAction::ChooseManaColor {
            choice: ManaChoice::SingleColor(ManaType::Green),
            count: 1,
        },
        acc,
    );
}

/// Row 1: a creature mana ability whose cost is `{T}, Exert` records exertion.
#[test]
fn exert_cost_on_a_creature_mana_ability_records_exertion() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let ritualist = scenario
        .add_creature_from_oracle(P0, "Oasis Ritualist", 2, 4, OASIS_RITUALIST)
        .id();
    let mut runner = scenario.build();
    untap(&mut runner, &[ritualist]);

    let idx = composite_index(&runner, ritualist);
    let mut events = Vec::new();
    activate_ritualist(&mut runner, ritualist, idx, &mut events);

    // Reach guard: the exert ability ran (two Green, tapped, one CantUntap).
    assert_eq!(pool_units(&runner, ManaType::Green), 2);
    assert!(runner.state().objects[&ritualist].tapped);
    assert_eq!(cant_untap_markers(&runner, ritualist), 1);
    // The fix.
    assert!(runner.state().exerted_this_turn.contains(&ritualist));
    assert_eq!(exerted_events(&events), vec![(ritualist, P0)]);
}

/// Row 1 control: the plain `{T}: Add one mana` ability exerts nothing.
#[test]
fn plain_tap_ability_does_not_exert() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let ritualist = scenario
        .add_creature_from_oracle(P0, "Oasis Ritualist", 2, 4, OASIS_RITUALIST)
        .id();
    let mut runner = scenario.build();
    untap(&mut runner, &[ritualist]);

    let idx = tap_only_index(&runner, ritualist);
    let mut events = Vec::new();
    activate_ritualist(&mut runner, ritualist, idx, &mut events);

    assert_eq!(pool_units(&runner, ManaType::Green), 1, "reach guard");
    assert!(runner.state().objects[&ritualist].tapped);
    assert!(runner.state().exerted_this_turn.is_empty());
    assert_eq!(cant_untap_markers(&runner, ritualist), 0);
    assert!(exerted_events(&events).is_empty());
}

fn arena_scenario(with_survivors: bool) -> (GameRunner, ObjectId, ObjectId) {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let arena = scenario
        .add_land_from_oracle(P0, "Arena of Glory", ARENA_OF_GLORY)
        .id();
    let mountain = scenario.add_basic_land(P0, ManaColor::Red);
    if with_survivors {
        scenario.add_creature_from_oracle(P0, "Resolute Survivors", 3, 3, RESOLUTE_SURVIVORS);
    }
    let mut runner = scenario.build();
    untap(&mut runner, &[arena, mountain]);
    (runner, arena, mountain)
}

/// Taps the Mountain for the `{R}` sub-cost, then exerts Arena of Glory.
fn exert_arena(runner: &mut GameRunner, arena: ObjectId, mountain: ObjectId) -> Vec<GameEvent> {
    let mut events = runner.activate(mountain, 0).resolve().events().to_vec();
    let idx = composite_index(runner, arena);
    events.extend(runner.activate(arena, idx).resolve().events().to_vec());
    events
}

/// Row 2: a land mana ability with a mana sub-cost (`{R}, {T}, Exert`).
#[test]
fn exert_cost_on_a_land_mana_ability_records_exertion() {
    let (mut runner, arena, mountain) = arena_scenario(false);
    let events = exert_arena(&mut runner, arena, mountain);

    // Reach guard: the exert ability ran (two Red in the pool, Arena tapped).
    assert_eq!(pool_units(&runner, ManaType::Red), 2);
    assert!(runner.state().objects[&arena].tapped);
    assert_eq!(cant_untap_markers(&runner, arena), 1);
    assert!(runner.state().exerted_this_turn.contains(&arena));
    assert_eq!(exerted_events(&events), vec![(arena, P0)]);
}

/// Row 3: a stack ability paid with `{T}, Exert` (Fervent Paincaster).
#[test]
fn exert_cost_on_a_stack_ability_records_exertion() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let paincaster = scenario
        .add_creature_from_oracle(P0, "Fervent Paincaster", 2, 2, FERVENT_PAINCASTER)
        .id();
    let bear = scenario.add_creature(P1, "Grizzly Bears", 2, 2).id();
    let mut runner = scenario.build();
    untap(&mut runner, &[paincaster]);

    let idx = composite_index(&runner, paincaster);
    let events = runner
        .activate(paincaster, idx)
        .target_object(bear)
        .resolve()
        .events()
        .to_vec();

    // Reach guard: the ability resolved.
    assert_eq!(runner.state().objects[&bear].damage_marked, 1);
    assert!(runner.state().objects[&paincaster].tapped);
    assert_eq!(cant_untap_markers(&runner, paincaster), 1);
    assert!(runner.state().exerted_this_turn.contains(&paincaster));
    assert_eq!(exerted_events(&events), vec![(paincaster, P0)]);
}

/// Row 3 control: the `{T}:` player-damage ability exerts nothing.
#[test]
fn paincasters_tap_only_ability_does_not_exert() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let paincaster = scenario
        .add_creature_from_oracle(P0, "Fervent Paincaster", 2, 2, FERVENT_PAINCASTER)
        .id();
    let mut runner = scenario.build();
    untap(&mut runner, &[paincaster]);

    let idx = tap_only_index(&runner, paincaster);
    let events = runner
        .activate(paincaster, idx)
        .target_player(P1)
        .resolve()
        .events()
        .to_vec();

    assert_eq!(life(&runner, P1), 19, "reach guard: the ability resolved");
    assert!(runner.state().exerted_this_turn.is_empty());
    assert!(exerted_events(&events).is_empty());
}

/// Row 4a: the observer fires for a stack-ability exert.
#[test]
fn survivors_fires_for_a_stack_ability_exert() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let paincaster = scenario
        .add_creature_from_oracle(P0, "Fervent Paincaster", 2, 2, FERVENT_PAINCASTER)
        .id();
    scenario.add_creature_from_oracle(P0, "Resolute Survivors", 3, 3, RESOLUTE_SURVIVORS);
    let bear = scenario.add_creature(P1, "Grizzly Bears", 2, 2).id();
    let mut runner = scenario.build();
    untap(&mut runner, &[paincaster]);

    let idx = composite_index(&runner, paincaster);
    let events = runner
        .activate(paincaster, idx)
        .target_object(bear)
        .resolve()
        .events()
        .to_vec();

    assert_eq!(exerted_events(&events).len(), 1, "reach guard: exert event");
    assert_eq!(life(&runner, P1), 19, "Survivors drained exactly once");
    assert_eq!(life(&runner, P0), 21);
}

/// Row 4b: the observer fires for a creature mana-ability exert at priority
/// (Arena of Glory is a land, so "exert a creature" does not cover it).
#[test]
fn survivors_fires_for_a_mana_ability_exert() {
    let (mut runner, ritualist) = ritualist_with_survivors();
    let idx = composite_index(&runner, ritualist);
    let mut events = Vec::new();
    activate_ritualist(&mut runner, ritualist, idx, &mut events);

    assert_eq!(exerted_events(&events).len(), 1, "reach guard: exert event");
    assert_eq!(runner.state().stack.len(), 1, "reach guard: trigger queued");
    runner.advance_until_stack_empty();
    assert_eq!(life(&runner, P1), 19, "Survivors drained exactly once");
    assert_eq!(life(&runner, P0), 21);
}

/// Row 4c: a land exerted as a cost is not "a creature", so the observer
/// stays quiet (negative pair of row 4b; the exert itself is recorded).
#[test]
fn survivors_ignores_a_land_exerted_as_a_cost() {
    let (mut runner, arena, mountain) = arena_scenario(true);
    let events = exert_arena(&mut runner, arena, mountain);

    assert_eq!(exerted_events(&events), vec![(arena, P0)], "reach guard");
    assert!(runner.state().stack.is_empty());
    runner.advance_until_stack_empty();
    assert_eq!(life(&runner, P1), 20);
    assert_eq!(life(&runner, P0), 20);
}

fn ritualist_with_survivors() -> (GameRunner, ObjectId) {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let ritualist = scenario
        .add_creature_from_oracle(P0, "Oasis Ritualist", 2, 4, OASIS_RITUALIST)
        .id();
    scenario.add_creature_from_oracle(P0, "Resolute Survivors", 3, 3, RESOLUTE_SURVIVORS);
    let mut runner = scenario.build();
    untap(&mut runner, &[ritualist]);
    (runner, ritualist)
}

/// Attack route helper: `attacker` (controlled by `controller`) attacks the
/// other player and the exert is answered with `exert`. Returns all events.
fn attack_with_exert(
    runner: &mut GameRunner,
    controller: PlayerId,
    attacker: ObjectId,
    exert: bool,
) -> Vec<GameEvent> {
    let defender = if controller == P0 { P1 } else { P0 };
    if controller != P0 {
        runner.state_mut().active_player = controller;
    }
    runner.advance_to_combat();
    assert_eq!(runner.waiting_for_kind(), "DeclareAttackers");
    let mut events = Vec::new();
    let result = runner
        .declare_attackers(&[(attacker, AttackTarget::Player(defender))])
        .expect("declare attackers");
    events.extend(result.events);
    assert_eq!(runner.waiting_for_kind(), "ExertChoice");
    act_collect(runner, GameAction::ChooseExert { exert }, &mut events);
    runner.advance_until_stack_empty();
    events
}

/// Row 5: the observer does not fire for an opponent's attack exert.
#[test]
fn survivors_does_not_fire_for_an_opponents_exert() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    scenario.add_creature_from_oracle(P0, "Resolute Survivors", 3, 3, RESOLUTE_SURVIVORS);
    let warrior = scenario
        .add_creature_from_oracle(P1, "Bitterblade Warrior", 2, 2, BITTERBLADE_WARRIOR)
        .id();
    let mut runner = scenario.build();

    let events = attack_with_exert(&mut runner, P1, warrior, true);

    // Reach guards: the opponent exerted, and its own linked trigger resolved.
    assert_eq!(exerted_events(&events), vec![(warrior, P1)]);
    assert!(runner.state().exerted_this_turn.contains(&warrior));
    assert_eq!(runner.state().objects[&warrior].power, Some(3));
    // The observer stayed quiet.
    assert_eq!(life(&runner, P0), 20);
    assert_eq!(life(&runner, P1), 20);
}

/// Row 6: the observer fires for another creature's attack exert, and the
/// self-linked trigger still resolves. Pair of row 5.
#[test]
fn survivors_fires_for_another_creatures_attack_exert() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    scenario.add_creature_from_oracle(P0, "Resolute Survivors", 3, 3, RESOLUTE_SURVIVORS);
    let warrior = scenario
        .add_creature_from_oracle(P0, "Bitterblade Warrior", 2, 2, BITTERBLADE_WARRIOR)
        .id();
    let mut runner = scenario.build();

    let events = attack_with_exert(&mut runner, P0, warrior, true);

    assert_eq!(exerted_events(&events), vec![(warrior, P0)]);
    assert_eq!(runner.state().objects[&warrior].power, Some(3));
    assert_eq!(life(&runner, P1), 19);
    assert_eq!(life(&runner, P0), 21);
}

/// Row 6 control: declining the exert does nothing.
#[test]
fn declining_the_exert_fires_no_observer() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    scenario.add_creature_from_oracle(P0, "Resolute Survivors", 3, 3, RESOLUTE_SURVIVORS);
    let warrior = scenario
        .add_creature_from_oracle(P0, "Bitterblade Warrior", 2, 2, BITTERBLADE_WARRIOR)
        .id();
    let mut runner = scenario.build();

    let events = attack_with_exert(&mut runner, P0, warrior, false);

    assert!(exerted_events(&events).is_empty());
    assert!(runner.state().exerted_this_turn.is_empty());
    assert_eq!(runner.state().objects[&warrior].power, Some(2));
    assert_eq!(life(&runner, P1), 20);
    assert_eq!(life(&runner, P0), 20);
}

/// Row 7: no double fire - Survivors exerting itself drains exactly once.
#[test]
fn survivors_exerting_itself_drains_once() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let survivors = scenario
        .add_creature_from_oracle(P0, "Resolute Survivors", 3, 3, RESOLUTE_SURVIVORS)
        .id();
    let mut runner = scenario.build();

    let events = attack_with_exert(&mut runner, P0, survivors, true);

    assert!(runner.state().exerted_this_turn.contains(&survivors));
    assert_eq!(exerted_events(&events), vec![(survivors, P0)]);
    assert_eq!(life(&runner, P1), 19, "one trigger, not two");
    assert_eq!(life(&runner, P0), 21);
}

/// Row 8, CR 701.43b: an already-exerted permanent can be exerted again; the
/// event, effect and observer trigger happen again.
#[test]
fn exerting_an_already_exerted_permanent_exerts_again() {
    let (mut runner, ritualist) = ritualist_with_survivors();
    let idx = composite_index(&runner, ritualist);
    let mut first = Vec::new();
    activate_ritualist(&mut runner, ritualist, idx, &mut first);
    runner.advance_until_stack_empty();
    untap(&mut runner, &[ritualist]);
    let mut second = Vec::new();
    activate_ritualist(&mut runner, ritualist, idx, &mut second);
    runner.advance_until_stack_empty();

    assert_eq!(exerted_events(&first).len(), 1);
    assert_eq!(exerted_events(&second), vec![(ritualist, P0)]);
    assert!(runner.state().exerted_this_turn.contains(&ritualist));
    assert_eq!(cant_untap_markers(&runner, ritualist), 2);
    assert_eq!(life(&runner, P1), 18, "one drain per exert");
    assert_eq!(life(&runner, P0), 22);
}

fn witness(scenario: &mut GameScenario, shards: &[ManaCostShard]) -> ObjectId {
    scenario
        .add_spell_to_hand_from_oracle(P0, "Exert Witness", false, "You gain 1 life.")
        .with_mana_cost(ManaCost::Cost {
            shards: shards.to_vec(),
            generic: 0,
        })
        .id()
}

/// Row 10: a mana ability with an exert cost activated inside a cast's payment
/// (CR 601.2h + CR 605.3b) records exertion once, and the observer fires once.
#[test]
fn exert_cost_mana_ability_inside_a_spells_payment() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let ritualist = scenario
        .add_creature_from_oracle(P0, "Oasis Ritualist", 2, 4, OASIS_RITUALIST)
        .id();
    scenario.add_creature_from_oracle(P0, "Resolute Survivors", 3, 3, RESOLUTE_SURVIVORS);
    let spell = witness(&mut scenario, &[ManaCostShard::Green, ManaCostShard::Green]);
    let mut runner = scenario.build();
    untap(&mut runner, &[ritualist]);

    let mut events = Vec::new();
    let card_id = runner.state().objects[&spell].card_id;
    act_collect(
        &mut runner,
        GameAction::CastSpell {
            object_id: spell,
            card_id,
            targets: vec![],
            payment_mode: CastPaymentMode::Manual,
        },
        &mut events,
    );
    assert!(matches!(
        runner.state().waiting_for,
        WaitingFor::ManaPayment { .. }
    ));
    let idx = composite_index(&runner, ritualist);
    activate_ritualist(&mut runner, ritualist, idx, &mut events);
    assert!(matches!(
        runner.state().waiting_for,
        WaitingFor::ManaPayment { .. }
    ));
    act_collect(&mut runner, GameAction::PassPriority, &mut events);
    runner.advance_until_stack_empty();

    // Reach guards: the spell was paid with the exert ability and resolved.
    assert_eq!(runner.state().objects[&spell].zone, Zone::Graveyard);
    assert_eq!(cant_untap_markers(&runner, ritualist), 1);
    // The fix.
    assert!(runner.state().exerted_this_turn.contains(&ritualist));
    assert_eq!(exerted_events(&events), vec![(ritualist, P0)]);
    assert_eq!(life(&runner, P1), 19, "exactly one observer trigger");
    assert_eq!(life(&runner, P0), 22, "witness +1, drain +1");
}

/// Row 11(a): auto-pay of `{G}` uses the plain row, so nothing is exerted.
#[test]
fn autopay_plain_row_does_not_exert() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let ritualist = scenario
        .add_creature_from_oracle(P0, "Oasis Ritualist", 2, 4, OASIS_RITUALIST)
        .id();
    scenario.add_creature_from_oracle(P0, "Resolute Survivors", 3, 3, RESOLUTE_SURVIVORS);
    let spell = witness(&mut scenario, &[ManaCostShard::Green]);
    let mut runner = scenario.build();
    untap(&mut runner, &[ritualist]);

    let outcome: CastOutcome = runner.cast(spell).resolve();

    // Reach guard: the witness resolved and the Ritualist paid for it.
    assert_eq!(outcome.zone_of(spell), Zone::Graveyard);
    assert!(outcome.state().objects[&ritualist].tapped);
    assert!(outcome.events().iter().any(|e| matches!(
        e,
        GameEvent::ManaAdded { source_id, .. } if *source_id == ritualist
    )));
    assert!(outcome.state().exerted_this_turn.is_empty());
    assert!(exerted_events(outcome.events()).is_empty());
    assert_eq!(outcome.life_delta(P0), 1);
    assert_eq!(outcome.life_delta(P1), 0);
}
