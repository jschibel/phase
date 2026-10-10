//! CR 608.2c + CR 608.2n: the controller follows an instant or sorcery
//! spell's instructions in order, and the spell is put into its owner's
//! graveyard "as the final part of" its resolution — after every instruction
//! has been followed, including an instruction that paused for a player's
//! choice (CR 608.2d). A spell whose instructions pause (a chosen discard, a
//! library search) must therefore stay on the stack until that choice is made,
//! and leave it only afterwards.
//!
//! Observable consequences, each asserted below:
//! * Faithless Looting ("Draw two cards, then discard two cards.") ends up ON
//!   TOP of the two discarded cards (CR 404.2 keeps graveyard order), not
//!   beneath them, and is reported resolved only then.
//! * Doomsday ("Search your library and graveyard for five cards and exile the
//!   rest.") is not in the graveyard while its own search resolves, so "the
//!   rest" never includes Doomsday itself.
//! * A spell with no pause still reaches the graveyard as it resolves, before
//!   anyone receives priority.
//! * Every rule that sends a resolving spell somewhere other than the
//!   graveyard still applies to the deferred final part: flashback (exile),
//!   rebound (exile, with its upkeep cast), an Adventure (exile, with the
//!   creature castable), buyback (its owner's hand), a CR 616.1 ordering of two
//!   graveyard replacements, and an "end the combat phase" spell's exile
//!   (CR 724.2b), decided by the phase the resolution began in.
//! * CR 800.4a: a player who leaves the game while their spell is paused takes
//!   it with them — its owner leaving exiles it (and its controller still
//!   finishes it, CR 608.2m), its controller leaving exiles it when that player
//!   still controls it — rather than the final part putting it into a
//!   graveyard. CR 608.2m: such a spell is reported resolved exactly once when
//!   a surviving player still had one of its instructions to answer; one whose
//!   departed player's question was its last live work is retired with that
//!   player and not reported resolved.
//! * The owed final part is state of the resolution carrier
//!   (`GameState::owed_spell_final_part`), never of the spell's ability, so a
//!   copy of a paused spell (CR 707.10) inherits none of it.
//! * A paused spell that its own instruction moves off the stack ("Exile
//!   Invoke Calamity", after its CR 608.2g free-cast window) is left where it
//!   was put, and is still reported resolved once.

use engine::game::scenario::{GameRunner, GameScenario, P0, P1};
use engine::game::scenario_db::GameScenarioDbExt;
use engine::types::ability::CastingPermission;
use engine::types::actions::GameAction;
use engine::types::events::GameEvent;
use engine::types::game_state::{
    CastOfferKind, CastPaymentMode, CastingVariant, GameState, OwedSpellFinalPart, StackEntryKind,
    WaitingFor,
};
use engine::types::identifiers::ObjectId;
use engine::types::mana::{ManaColor, ManaCost, ManaType, ManaUnit};
use engine::types::phase::Phase;
use engine::types::player::PlayerId;
use engine::types::zones::Zone;

use crate::support::shared_card_db as load_db;

const SHOCK: &str = "Shock deals 2 damage to any target.";

fn db() -> &'static engine::database::card_db::CardDatabase {
    load_db().expect("the committed integration card fixture loads")
}

fn mana(color: ManaType, amount: usize) -> Vec<ManaUnit> {
    (0..amount)
        .map(|_| ManaUnit::new(color, ObjectId(0), false, vec![]))
        .collect()
}

fn graveyard(state: &GameState, player: PlayerId) -> Vec<ObjectId> {
    state
        .players
        .iter()
        .find(|p| p.id == player)
        .expect("player must exist")
        .graveyard
        .iter()
        .copied()
        .collect()
}

fn stack_resolved_count(events: &[GameEvent], spell: ObjectId) -> usize {
    events
        .iter()
        .filter(
            |event| matches!(event, GameEvent::StackResolved { object_id } if *object_id == spell),
        )
        .count()
}

struct LootingBoard {
    runner: GameRunner,
    looting: ObjectId,
    discard_a: ObjectId,
    discard_b: ObjectId,
}

fn stage_faithless_looting() -> LootingBoard {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let looting = scenario.add_real_card(P0, "Faithless Looting", Zone::Hand, db());
    let discard_a = scenario.add_card_to_hand(P0, "Discard A");
    let discard_b = scenario.add_card_to_hand(P0, "Discard B");
    scenario.with_library_top(P0, &["Draw One", "Draw Two", "Pad A", "Pad B"]);
    scenario.add_basic_land(P0, ManaColor::Red);
    let runner = scenario.build();
    LootingBoard {
        runner,
        looting,
        discard_a,
        discard_b,
    }
}

/// CR 608.2n + CR 404.2: the spell reaches the graveyard after the cards its
/// own discard instruction put there, so it is the top card of the graveyard.
#[test]
fn faithless_looting_is_put_into_the_graveyard_on_top_of_the_cards_it_discarded() {
    let LootingBoard {
        mut runner,
        looting,
        discard_a,
        discard_b,
    } = stage_faithless_looting();

    let outcome = runner
        .cast(looting)
        .discard(&[discard_a, discard_b])
        .resolve();

    assert!(
        matches!(outcome.final_waiting_for(), WaitingFor::Priority { .. }),
        "the resolution must complete, got {:?}",
        outcome.final_waiting_for()
    );
    outcome.assert_hand_drawn(P0, 0);
    outcome.assert_zone(&[discard_a, discard_b, looting], Zone::Graveyard);
    assert_eq!(
        graveyard(outcome.state(), P0),
        vec![discard_a, discard_b, looting],
        "CR 608.2n: the spell is put into its owner's graveyard as the FINAL part of its \
         resolution, after the discard it instructs — so it sits on top of the discarded cards"
    );
    assert_eq!(
        stack_resolved_count(outcome.events(), looting),
        1,
        "the resolution is reported exactly once"
    );
}

/// CR 608.2c + CR 608.2d + CR 608.2n: while the spell's discard instruction
/// waits for the player's choice, the spell has not finished resolving and is
/// still on the stack — it is not yet a card in the graveyard, and it is not
/// yet reported resolved.
#[test]
fn a_spell_paused_on_its_own_discard_choice_is_still_on_the_stack() {
    let LootingBoard {
        mut runner,
        looting,
        discard_a,
        discard_b,
    } = stage_faithless_looting();

    let mut cast = runner.cast(looting).commit();
    let mut events_until_pause = Vec::new();
    for _ in 0..2 {
        if !matches!(cast.state().waiting_for, WaitingFor::Priority { .. }) {
            break;
        }
        let result = cast
            .act(GameAction::PassPriority)
            .expect("passing priority resolves Faithless Looting");
        events_until_pause.extend(result.events);
    }
    assert!(
        matches!(cast.state().waiting_for, WaitingFor::DiscardChoice { .. }),
        "reach guard: the discard instruction pauses for the choice, got {:?}",
        cast.state().waiting_for
    );
    assert_eq!(
        cast.state().objects[&looting].zone,
        Zone::Stack,
        "CR 608.2c + CR 608.2n: a spell is put into the graveyard only as the final part of \
         its resolution, not before its discard is chosen"
    );
    assert!(
        !graveyard(cast.state(), P0).contains(&looting),
        "the paused spell is not a card in the graveyard"
    );
    assert_eq!(
        stack_resolved_count(&events_until_pause, looting),
        0,
        "a spell still resolving is not reported resolved"
    );

    let answered = cast
        .act(GameAction::SelectCards {
            cards: vec![discard_a, discard_b],
        })
        .expect("choosing the two discards finishes the resolution");
    assert_eq!(
        stack_resolved_count(&answered.events, looting),
        1,
        "the resolution is reported when its final part runs"
    );

    assert!(
        matches!(cast.state().waiting_for, WaitingFor::Priority { .. }),
        "the resolution completes once the choice is made, got {:?}",
        cast.state().waiting_for
    );
    assert_eq!(
        graveyard(cast.state(), P0),
        vec![discard_a, discard_b, looting],
        "CR 608.2n: the spell follows its discards into the graveyard"
    );
}

/// CR 608.2n + CR 701.23a: Doomsday is still on the stack while its own search
/// resolves, so "exile the rest" of the searched graveyard never exiles
/// Doomsday; it reaches the graveyard afterwards as the final part of its
/// resolution.
#[test]
fn doomsday_is_not_exiled_by_its_own_search_and_ends_in_the_graveyard() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    scenario.with_library_top(
        P0,
        &[
            "Library A",
            "Library B",
            "Library C",
            "Library D",
            "Library E",
            "Library F",
            "Library G",
        ],
    );
    scenario.with_graveyard(P0, &["Graveyard A", "Graveyard B", "Graveyard C"]);
    let doomsday = scenario.add_real_card(P0, "Doomsday", Zone::Hand, db());
    scenario.with_mana_pool(P0, mana(ManaType::Black, 3));
    let mut runner = scenario.build();
    let searched: Vec<ObjectId> = runner
        .state()
        .objects
        .iter()
        .filter(|(_, object)| {
            object.owner == P0 && matches!(object.zone, Zone::Library | Zone::Graveyard)
        })
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(
        searched.len(),
        10,
        "seven library cards and three graveyard cards"
    );

    let outcome = runner.cast(doomsday).search_first_legal().resolve();

    assert!(
        matches!(outcome.final_waiting_for(), WaitingFor::Priority { .. }),
        "the resolution must complete, got {:?}",
        outcome.final_waiting_for()
    );
    outcome.assert_life_delta(P0, -10);
    assert_eq!(
        outcome.zone_of(doomsday),
        Zone::Graveyard,
        "CR 608.2n: Doomsday is put into its owner's graveyard as the final part of its \
         resolution — it was not among 'the rest' its own search exiled"
    );
    assert_eq!(
        graveyard(outcome.state(), P0),
        vec![doomsday],
        "the searched graveyard was emptied (chosen cards to the library, the rest to exile) \
         before Doomsday itself arrived"
    );
    let exiled: Vec<ObjectId> = searched
        .iter()
        .copied()
        .filter(|id| outcome.zone_of(*id) == Zone::Exile)
        .collect();
    assert_eq!(
        exiled.len(),
        5,
        "exactly the five unchosen searched cards are exiled"
    );
    let on_top: Vec<ObjectId> = outcome.state().players[P0.0 as usize]
        .library
        .iter()
        .take(5)
        .copied()
        .collect();
    assert!(
        on_top.iter().all(|id| searched.contains(id)),
        "the five chosen cards are on top of the library"
    );
}

/// CR 608.2n: a spell whose instructions never pause goes to the graveyard as it
/// resolves, before any player receives priority — the deferral is confined to
/// resolutions that actually paused.
#[test]
fn a_spell_with_no_paused_instruction_reaches_the_graveyard_as_it_resolves() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let shock = scenario
        .add_spell_to_hand_from_oracle(P0, "Shock", true, SHOCK)
        .id();
    scenario.add_basic_land(P0, ManaColor::Red);
    let mut runner = scenario.build();

    let outcome = runner.cast(shock).target_player(P1).resolve();

    assert!(
        matches!(outcome.final_waiting_for(), WaitingFor::Priority { .. }),
        "got {:?}",
        outcome.final_waiting_for()
    );
    outcome.assert_life_delta(P1, -2);
    outcome.assert_zone(&[shock], Zone::Graveyard);
    assert_eq!(graveyard(outcome.state(), P0), vec![shock]);
    assert_eq!(stack_resolved_count(outcome.events(), shock), 1);
}

/// Pass priority and answer the resolution's questions until a player holds
/// priority over an empty stack, returning every event. `answer` takes the
/// prompts a test cares about; a discard or a search otherwise takes the
/// first cards offered.
fn run_until_settled(
    runner: &mut GameRunner,
    mut answer: impl FnMut(&GameState) -> Option<GameAction>,
) -> Vec<GameEvent> {
    let mut events = Vec::new();
    for _ in 0..40 {
        let state = runner.state();
        let action = match answer(state) {
            Some(action) => action,
            None => match &state.waiting_for {
                WaitingFor::Priority { .. } if state.stack.is_empty() => return events,
                WaitingFor::Priority { .. } => GameAction::PassPriority,
                WaitingFor::DiscardChoice { cards, count, .. }
                | WaitingFor::SearchChoice { cards, count, .. } => GameAction::SelectCards {
                    cards: cards.iter().copied().take(*count).collect(),
                },
                other => panic!("unexpected prompt while resolving: {other:?}"),
            },
        };
        events.extend(
            runner
                .act(action)
                .expect("the engine accepts the answer")
                .events,
        );
    }
    panic!(
        "the resolution did not settle; waiting for {:?}",
        runner.state().waiting_for
    );
}

/// The resolution is over: priority over an empty stack, no carrier left, and
/// no object stranded in the stack zone.
fn assert_settled(state: &GameState) {
    assert!(
        matches!(state.waiting_for, WaitingFor::Priority { .. }),
        "the game is back at a priority window, got {:?}",
        state.waiting_for
    );
    assert!(state.stack.is_empty(), "the stack is empty");
    assert!(
        state.resolving_stack_entry.is_none(),
        "the resolution carrier was retired"
    );
    assert!(
        state.owed_spell_final_part().is_none(),
        "the owed final part ends with its carrier"
    );
    let stranded: Vec<&str> = state
        .objects
        .values()
        .filter(|object| object.zone == Zone::Stack)
        .map(|object| object.name.as_str())
        .collect();
    assert!(
        stranded.is_empty(),
        "no object is left in the stack zone: {stranded:?}"
    );
}

/// CR 702.34a + CR 608.2n: a spell cast with flashback is exiled "instead of
/// putting it anywhere else any time it would leave the stack" — including
/// when it leaves as the deferred final part of a paused resolution.
#[test]
fn a_flashback_spell_paused_on_its_discard_is_exiled_as_its_final_part() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let looting = scenario.add_real_card(P0, "Faithless Looting", Zone::Graveyard, db());
    scenario.add_card_to_hand(P0, "Discard A");
    scenario.add_card_to_hand(P0, "Discard B");
    scenario.with_library_top(P0, &["Draw One", "Draw Two", "Pad"]);
    scenario.with_mana_pool(P0, mana(ManaType::Red, 3));
    let mut runner = scenario.build();

    runner
        .cast(looting)
        .casting_variant(CastingVariant::Flashback)
        .commit();
    let events = run_until_settled(&mut runner, |_| None);

    let state = runner.state();
    assert_eq!(
        state.objects[&looting].zone,
        Zone::Exile,
        "CR 702.34a: the flashback spell is exiled as it leaves the stack"
    );
    assert_eq!(
        graveyard(state, P0).len(),
        2,
        "only the two discarded cards are in the graveyard"
    );
    assert_eq!(stack_resolved_count(&events, looting), 1);
    assert_settled(state);
}

/// CR 702.88a + CR 608.2n: a rebound spell cast from hand is exiled "instead
/// of putting it into your graveyard as it resolves", and its upkeep cast is
/// set up — also when its own search paused the resolution.
#[test]
fn a_rebound_spell_paused_on_its_search_is_exiled_with_its_upkeep_cast() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let terramorph = scenario.add_real_card(P0, "Terramorph", Zone::Hand, db());
    let forest = scenario.add_real_card(P0, "Forest", Zone::Library, db());
    scenario.with_mana_pool(P0, mana(ManaType::Green, 4));
    let mut runner = scenario.build();

    runner.cast(terramorph).commit();
    let events = run_until_settled(&mut runner, |_| None);

    let state = runner.state();
    assert_eq!(
        state.objects[&forest].zone,
        Zone::Battlefield,
        "reach guard: the paused search put the land onto the battlefield"
    );
    assert_eq!(
        state.objects[&terramorph].zone,
        Zone::Exile,
        "CR 702.88a: the rebound spell is exiled instead of going to the graveyard"
    );
    assert!(
        state
            .delayed_triggers
            .iter()
            .any(|trigger| trigger.source_id == terramorph),
        "CR 702.88a: the next-upkeep cast from exile is set up"
    );
    assert_eq!(stack_resolved_count(&events, terramorph), 1);
    assert_settled(state);
}

/// CR 715.3d + CR 608.2n: an Adventure spell is exiled instead of going to the
/// graveyard as it resolves, and its creature may then be cast from exile —
/// also when the Adventure's own search paused the resolution.
#[test]
fn an_adventure_paused_on_its_search_is_exiled_with_its_creature_castable() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let giant = scenario.add_real_card(P0, "Beanstalk Giant", Zone::Hand, db());
    let forest = scenario.add_real_card(P0, "Forest", Zone::Library, db());
    scenario.with_mana_pool(P0, mana(ManaType::Green, 3));
    let mut runner = scenario.build();

    runner.cast(giant).adventure_face(false).commit();
    let events = run_until_settled(&mut runner, |_| None);

    let state = runner.state();
    assert_eq!(
        state.objects[&forest].zone,
        Zone::Battlefield,
        "reach guard: Fertile Footsteps' search put the land onto the battlefield"
    );
    assert_eq!(
        state.objects[&giant].zone,
        Zone::Exile,
        "CR 715.3d: the Adventure spell is exiled as it resolves"
    );
    assert!(
        state.objects[&giant]
            .casting_permissions
            .iter()
            .any(|permission| matches!(permission, CastingPermission::AdventureCreature)),
        "CR 715.3d: its controller may cast the creature from exile"
    );
    assert_eq!(stack_resolved_count(&events, giant), 1);
    assert_settled(state);
}

/// CR 702.27a + CR 608.2n: a spell whose buyback cost was paid is put into its
/// owner's hand "instead of into that player's graveyard as it resolves" —
/// also when its own search paused the resolution.
#[test]
fn a_buyback_spell_paused_on_its_search_returns_to_its_owners_hand() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let collusion = scenario.add_real_card(P0, "Demonic Collusion", Zone::Hand, db());
    let discard_a = scenario.add_card_to_hand(P0, "Discard A");
    let discard_b = scenario.add_card_to_hand(P0, "Discard B");
    scenario.with_library_top(P0, &["Library A", "Library B"]);
    scenario.with_mana_pool(P0, mana(ManaType::Black, 5));
    let mut runner = scenario.build();

    runner
        .cast(collusion)
        .accept_optional()
        .pay_cost_with(&[discard_a, discard_b])
        .commit();
    let events = run_until_settled(&mut runner, |_| None);

    let state = runner.state();
    assert_eq!(
        graveyard(state, P0),
        vec![discard_a, discard_b],
        "reach guard: the buyback cost was paid"
    );
    assert_eq!(
        state.objects[&collusion].zone,
        Zone::Hand,
        "CR 702.27a: the buyback spell returns to its owner's hand"
    );
    assert_eq!(stack_resolved_count(&events, collusion), 1);
    assert_settled(state);
}

/// CR 616.1 + CR 608.2n: when two replacement effects would both redirect the
/// spell's deferred move to the graveyard (Rest in Peace and Leyline of the
/// Void), the affected player orders them at that moment — after the paused
/// discard — and the spell then leaves the stack for exile.
#[test]
fn a_replacement_ordering_choice_on_the_deferred_move_is_asked_and_completed() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    scenario.add_real_card(P1, "Rest in Peace", Zone::Battlefield, db());
    scenario.add_real_card(P1, "Leyline of the Void", Zone::Battlefield, db());
    let looting = scenario.add_real_card(P0, "Faithless Looting", Zone::Hand, db());
    let discard_a = scenario.add_card_to_hand(P0, "Discard A");
    let discard_b = scenario.add_card_to_hand(P0, "Discard B");
    scenario.with_library_top(P0, &["Draw One", "Draw Two", "Pad"]);
    scenario.with_mana_pool(P0, mana(ManaType::Red, 1));
    let mut runner = scenario.build();

    runner.cast(looting).commit();
    let mut ordered_the_spells_own_move = false;
    let events = run_until_settled(&mut runner, |state| {
        matches!(state.waiting_for, WaitingFor::ReplacementChoice { .. }).then(|| {
            // Once the discards are delivered, the question left is the
            // spell's own final part.
            if state.objects[&looting].zone == Zone::Stack
                && [discard_a, discard_b]
                    .iter()
                    .all(|card| state.objects[card].zone == Zone::Exile)
            {
                ordered_the_spells_own_move = true;
                // The final part has run and parked its own CR 616.1 question:
                // it is no longer owed, but its carrier stays until answered.
                assert!(
                    state.owed_spell_final_part().is_none(),
                    "a final part that parked its own question is no longer owed"
                );
                assert_eq!(
                    state.resolving_stack_entry.as_ref().map(|entry| entry.id),
                    Some(looting),
                    "the carrier stays installed until the parked question is answered"
                );
            }
            GameAction::ChooseReplacement { index: 0 }
        })
    });

    assert!(
        ordered_the_spells_own_move,
        "CR 616.1: the deferred move asks its replacement-ordering question"
    );
    let state = runner.state();
    assert_eq!(state.objects[&looting].zone, Zone::Exile);
    assert!(graveyard(state, P0).is_empty());
    assert_eq!(stack_resolved_count(&events, looting), 1);
    assert_settled(state);
}

/// CR 724.2b + CR 608.2n: "end the combat phase" exiles every object on the
/// stack, including the resolving one — when the procedure ran during combat.
/// The spell's later discard pauses the resolution after the turn has moved to
/// the postcombat main phase; the deferred final part still exiles it, because
/// the phase that matters is the one its resolution began in.
#[test]
fn an_end_the_combat_phase_spell_paused_after_combat_ended_is_still_exiled() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::DeclareAttackers);
    let interrupt = scenario
        .add_spell_to_hand_from_oracle(
            P0,
            "Combat Interrupt",
            true,
            "End the combat phase. Draw two cards, then discard two cards.",
        )
        .id();
    scenario.add_card_to_hand(P0, "Discard A");
    scenario.add_card_to_hand(P0, "Discard B");
    scenario.with_library_top(P0, &["Draw One", "Draw Two", "Pad"]);
    let mut runner = scenario.build();

    runner.cast(interrupt).commit();
    let mut paused_after_combat = false;
    run_until_settled(&mut runner, |state| {
        if matches!(state.waiting_for, WaitingFor::DiscardChoice { .. }) {
            paused_after_combat = !state.phase.is_combat();
            let latched = state
                .owed_spell_final_part()
                .map(|owed| owed.resolution_start_phase);
            assert_eq!(
                latched,
                Some(Phase::DeclareAttackers),
                "the carrier latches the phase the resolution began in"
            );
        }
        None
    });

    assert!(
        paused_after_combat,
        "reach guard: the discard paused the resolution after combat had ended"
    );
    let state = runner.state();
    assert_eq!(
        state.objects[&interrupt].zone,
        Zone::Exile,
        "CR 724.2b: the resolving object is exiled by the end-of-combat procedure"
    );
    assert_eq!(graveyard(state, P0).len(), 2, "only the discards");
    assert_settled(state);
}

/// Faithless Looting cast by P0 in a three-player game, paused on P0's discard.
fn looting_paused_on_its_discard_in_a_three_player_game() -> (GameRunner, ObjectId) {
    let mut scenario = GameScenario::new_n_player(3, 42);
    scenario.at_phase(Phase::PreCombatMain);
    let looting = scenario.add_real_card(P0, "Faithless Looting", Zone::Hand, db());
    scenario.add_card_to_hand(P0, "Discard A");
    scenario.add_card_to_hand(P0, "Discard B");
    scenario.with_library_top(P0, &["Draw One", "Draw Two", "Pad"]);
    scenario.with_mana_pool(P0, mana(ManaType::Red, 1));
    let mut runner = scenario.build();

    runner.cast(looting).commit();
    pass_until_a_prompt(&mut runner);
    assert!(
        matches!(
            runner.state().waiting_for,
            WaitingFor::DiscardChoice { player, .. } if player == P0
        ),
        "reach guard: the resolution paused on P0's discard, got {:?}",
        runner.state().waiting_for
    );
    (runner, looting)
}

/// Pass priority around the table until something other than a priority
/// window is asked.
fn pass_until_a_prompt(runner: &mut GameRunner) {
    for _ in 0..12 {
        if !matches!(runner.state().waiting_for, WaitingFor::Priority { .. }) {
            return;
        }
        runner
            .act(GameAction::PassPriority)
            .expect("passing priority is accepted");
    }
}

fn concede(runner: &mut GameRunner, player: PlayerId) -> Vec<GameEvent> {
    engine::game::engine::apply(
        runner.state_mut(),
        player,
        GameAction::Concede { player_id: player },
    )
    .expect("a player may concede at any time")
    .events
}

/// CR 800.4a: when the player who owns and controls a paused spell leaves the
/// game, the spell leaves with every other object they own. Its deferred final
/// part must not later put it into the departed player's graveyard.
#[test]
fn a_paused_spell_leaves_the_game_with_its_departing_owner() {
    let (mut runner, looting) = looting_paused_on_its_discard_in_a_three_player_game();

    let mut events = concede(&mut runner, P0);
    events.extend(run_until_settled(&mut runner, |_| None));

    let state = runner.state();
    assert!(
        state.players[P0.0 as usize].hand.is_empty(),
        "reach guard: the departed player's cards left the game"
    );
    assert_eq!(
        state.objects[&looting].zone,
        Zone::Exile,
        "CR 800.4a: the departed player's spell leaves the game with their other cards"
    );
    assert!(graveyard(state, P0).is_empty());
    assert_eq!(
        stack_resolved_count(&events, looting),
        0,
        "a spell whose departing player's question was its last live work is retired with that \
         player and not reported resolved"
    );
    assert_settled(state);
}

/// Faithless Looting cast by P0 and stolen with Commandeer by P1, in a
/// three-player game, paused on its new controller's discard.
fn stolen_looting_paused_on_its_controllers_discard() -> (GameRunner, ObjectId) {
    let mut scenario = GameScenario::new_n_player(3, 42);
    scenario.at_phase(Phase::PreCombatMain);
    let looting = scenario.add_real_card(P0, "Faithless Looting", Zone::Hand, db());
    let commandeer = scenario.add_real_card(P1, "Commandeer", Zone::Hand, db());
    scenario.add_card_to_hand(P1, "P1 Discard A");
    scenario.add_card_to_hand(P1, "P1 Discard B");
    scenario.with_library_top(P1, &["P1 Draw One", "P1 Draw Two", "P1 Pad"]);
    scenario.with_mana_pool(P0, mana(ManaType::Red, 1));
    scenario.with_mana_pool(P1, mana(ManaType::Blue, 7));
    let mut runner = scenario.build();

    runner.cast(looting).commit();
    runner
        .act(GameAction::PassPriority)
        .expect("P0 passes priority to P1");
    runner.cast(commandeer).target_object(looting).commit();
    pass_until_a_prompt(&mut runner);
    // "You may choose new targets for it." Faithless Looting has none.
    if matches!(
        runner.state().waiting_for,
        WaitingFor::OptionalEffectChoice { .. }
    ) {
        runner
            .act(GameAction::DecideOptionalEffect { accept: false })
            .expect("P1 keeps the spell's targets");
        pass_until_a_prompt(&mut runner);
    }
    assert!(
        matches!(
            runner.state().waiting_for,
            WaitingFor::DiscardChoice { player, .. } if player == P1
        ),
        "reach guard: the stolen Looting paused on its new controller's discard, got {:?}",
        runner.state().waiting_for
    );
    (runner, looting)
}

/// CR 800.4a + CR 608.2m: when a paused spell's OWNER leaves while another
/// player controls it (Commandeer), the spell leaves the game, but the
/// resolution its controller started keeps going: that player still makes the
/// discard, there is nothing left for the final part to move, and the spell is
/// reported resolved once its controller has finished it.
#[test]
fn a_paused_spell_whose_owner_leaves_still_finishes_for_its_controller() {
    let (mut runner, looting) = stolen_looting_paused_on_its_controllers_discard();

    let mut events = concede(&mut runner, P0);
    assert_eq!(
        runner.state().objects[&looting].zone,
        Zone::Exile,
        "CR 800.4a: the spell leaves the game with its departing owner"
    );
    assert!(
        matches!(
            runner.state().waiting_for,
            WaitingFor::DiscardChoice { player, .. } if player == P1
        ),
        "CR 608.2m: the controller's resolution goes on, got {:?}",
        runner.state().waiting_for
    );

    events.extend(run_until_settled(&mut runner, |_| None));
    let state = runner.state();
    assert_eq!(
        graveyard(state, P1).len(),
        3,
        "Commandeer and the two cards its controller discarded"
    );
    assert!(!graveyard(state, P1).contains(&looting));
    assert_eq!(state.objects[&looting].zone, Zone::Exile);
    assert_eq!(stack_resolved_count(&events, looting), 1);
    assert_settled(state);
}

/// CR 800.4a: when the CONTROLLER of a paused spell it stole leaves, the
/// control effect ends and the spell reverts to its owner, so it is not among
/// the objects "still controlled by that player" and is not exiled. The
/// departed player's discard is gone; the resolution ends, and the final part
/// puts the spell into its owner's graveyard (CR 608.2n).
#[test]
fn a_paused_spell_whose_thief_leaves_reverts_to_its_owner() {
    let (mut runner, looting) = stolen_looting_paused_on_its_controllers_discard();

    let mut events = concede(&mut runner, P1);
    events.extend(run_until_settled(&mut runner, |_| None));

    let state = runner.state();
    assert_eq!(
        state.objects[&looting].zone,
        Zone::Graveyard,
        "CR 800.4a: the reverted spell is not exiled; it finishes into its owner's graveyard"
    );
    assert_eq!(graveyard(state, P0), vec![looting]);
    assert_eq!(stack_resolved_count(&events, looting), 1);
    assert_settled(state);
}

/// CR 800.4a: a paused spell whose controller leaves while STILL controlling it
/// — no control effect to end, because the controller cast a card another
/// player owns (Siphon Insight lets P0 cast P1's Faithless Looting from exile)
/// — is exiled, not put into its owner's graveyard by its final part.
#[test]
fn a_paused_spell_its_departing_controller_still_controls_is_exiled() {
    let mut scenario = GameScenario::new_n_player(3, 42);
    scenario.at_phase(Phase::PreCombatMain);
    let siphon = scenario.add_real_card(P0, "Siphon Insight", Zone::Hand, db());
    let looting = scenario.add_real_card(P1, "Faithless Looting", Zone::Library, db());
    scenario.with_library_top(P1, &["P1 Pad"]);
    scenario.add_card_to_hand(P0, "Discard A");
    scenario.add_card_to_hand(P0, "Discard B");
    scenario.with_library_top(P0, &["Draw One", "Draw Two", "Pad"]);
    scenario.with_mana_pool(
        P0,
        [ManaType::Blue, ManaType::Black, ManaType::Red]
            .into_iter()
            .flat_map(|color| mana(color, 1))
            .collect(),
    );
    let mut runner = scenario.build();

    runner.cast(siphon).target_player(P1).commit();
    run_until_settled(&mut runner, |state| {
        matches!(state.waiting_for, WaitingFor::DigChoice { .. }).then(|| GameAction::SelectCards {
            cards: vec![looting],
        })
    });
    assert_eq!(
        runner.state().objects[&looting].zone,
        Zone::Exile,
        "reach guard: Siphon Insight exiled P1's Faithless Looting for P0 to play"
    );

    runner.cast(looting).commit();
    pass_until_a_prompt(&mut runner);
    let state = runner.state();
    assert!(
        matches!(state.waiting_for, WaitingFor::DiscardChoice { player, .. } if player == P0),
        "reach guard: P0's Looting paused on P0's discard, got {:?}",
        state.waiting_for
    );
    assert_eq!(
        (
            state.objects[&looting].owner,
            state.objects[&looting].controller
        ),
        (P1, P0),
        "reach guard: P1 owns the paused spell and P0 controls it"
    );

    let mut events = concede(&mut runner, P0);
    events.extend(run_until_settled(&mut runner, |_| None));

    let state = runner.state();
    assert_eq!(
        state.objects[&looting].zone,
        Zone::Exile,
        "CR 800.4a: an object still controlled by the departed player is exiled"
    );
    assert!(
        !graveyard(state, P1).contains(&looting),
        "the final part does not put it into its owner's graveyard"
    );
    assert_eq!(stack_resolved_count(&events, looting), 0);
    assert_settled(state);
}

const INVOKE_CALAMITY: &str = "You may cast up to two instant and/or sorcery spells with \
     total mana value 6 or less from your graveyard and/or hand without paying their mana costs. \
     If those spells would be put into your graveyard, exile them instead. Exile Invoke Calamity.";

/// CR 608.2m + CR 608.2n: Invoke Calamity pauses on its free-cast window
/// (CR 608.2g), and its last instruction exiles it. Its final part then has no
/// move left to make — the spell stays where its instruction put it — but the
/// spell did resolve, and is reported resolved exactly once.
#[test]
fn a_paused_spell_that_exiles_itself_is_left_in_exile_and_reported_resolved() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let invoke = scenario
        .add_spell_to_hand_from_oracle(P0, "Invoke Calamity", true, INVOKE_CALAMITY)
        .with_mana_cost(ManaCost::generic(1))
        .id();
    scenario
        .add_spell_to_graveyard(P0, "Graveyard Bolt", true)
        .with_mana_cost(ManaCost::generic(2))
        .from_oracle_text("Draw a card.");
    scenario
        .add_spell_to_hand(P0, "Hand Divination", false)
        .with_mana_cost(ManaCost::generic(3))
        .from_oracle_text("Draw a card.");
    scenario.with_mana_pool(P0, mana(ManaType::Colorless, 1));
    let mut runner = scenario.build();

    let card_id = runner.state().objects[&invoke].card_id;
    let mut events = runner
        .act(GameAction::CastSpell {
            object_id: invoke,
            card_id,
            targets: vec![],
            payment_mode: CastPaymentMode::Auto,
        })
        .expect("casting Invoke Calamity is accepted")
        .events;
    for _ in 0..2 {
        events.extend(
            runner
                .act(GameAction::PassPriority)
                .expect("passing priority is accepted")
                .events,
        );
    }
    assert!(
        matches!(
            runner.state().waiting_for,
            WaitingFor::CastOffer {
                kind: CastOfferKind::FreeCastWindow { .. },
                ..
            }
        ),
        "reach guard: Invoke Calamity paused on its free-cast window, got {:?}",
        runner.state().waiting_for
    );
    assert_eq!(runner.state().objects[&invoke].zone, Zone::Stack);
    assert_eq!(stack_resolved_count(&events, invoke), 0);

    events.extend(
        runner
            .act(GameAction::FreeCastWindowChoice { selection: None })
            .expect("declining the free casts is accepted")
            .events,
    );
    events.extend(run_until_settled(&mut runner, |_| None));

    let state = runner.state();
    assert_eq!(
        state.objects[&invoke].zone,
        Zone::Exile,
        "the spell stays where its own instruction put it"
    );
    assert!(!graveyard(state, P0).contains(&invoke));
    assert_eq!(stack_resolved_count(&events, invoke), 1);
    assert_settled(state);
}

/// CR 707.10: a copy copies the spell and the choices made casting it; the
/// state of the original's resolution is not one of those. Chain of Smog's
/// resolution pauses on its target's discard and its copy offer, so its final
/// part is owed by its carrier — and the copy it makes while paused must carry
/// none of that. The copy's own resolution later owes its own final part.
#[test]
fn a_copy_of_a_paused_spell_inherits_no_owed_final_part() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let smog = scenario.add_real_card(P0, "Chain of Smog", Zone::Hand, db());
    for name in ["P1 A", "P1 B", "P1 C"] {
        scenario.add_card_to_hand(P1, name);
    }
    scenario.add_card_to_hand(P0, "P0 A");
    scenario.add_card_to_hand(P0, "P0 B");
    scenario.with_mana_pool(P0, mana(ManaType::Black, 2));
    let mut runner = scenario.build();

    runner.cast(smog).target_player(P1).commit();
    let announced = runner
        .state()
        .stack
        .iter()
        .find(|entry| entry.id == smog)
        .and_then(|entry| match &entry.kind {
            StackEntryKind::Spell {
                ability: Some(ability),
                ..
            } => Some(ability.context.clone()),
            _ => None,
        })
        .expect("Chain of Smog is on the stack with its announced ability");
    // CR 707.10: `copy_spell` clears exactly these cast-only fields on a copy;
    // everything else of the announced context is copied as it was.
    let mut expected_copy_context = announced;
    expected_copy_context.cast_from_zone = None;
    expected_copy_context.alt_cost_grant_source = None;
    expected_copy_context.graveyard_permission_authority = None;
    expected_copy_context.graveyard_permission_latch = None;

    let mut events = Vec::new();
    let mut copy: Option<ObjectId> = None;
    let mut smog_owed_at_its_discard = false;
    let mut copy_context_checked = false;
    let mut copy_owed_at_its_offer = false;
    for _ in 0..40 {
        let state = runner.state();
        let action = match &state.waiting_for {
            WaitingFor::Priority { .. } if state.stack.is_empty() => break,
            WaitingFor::Priority { .. } => GameAction::PassPriority,
            WaitingFor::DiscardChoice {
                player,
                cards,
                count,
                ..
            } => {
                if copy.is_none() {
                    assert_eq!(*player, P1, "Chain of Smog's target discards");
                    assert_eq!(
                        state.owed_spell_final_part(),
                        Some(&OwedSpellFinalPart {
                            spell: smog,
                            resolution_start_phase: Phase::PreCombatMain,
                        }),
                        "the paused Chain of Smog's carrier owes its final part"
                    );
                    smog_owed_at_its_discard = true;
                }
                GameAction::SelectCards {
                    cards: cards.iter().copied().take(*count).collect(),
                }
            }
            WaitingFor::OptionalEffectChoice { player, .. } => {
                assert_eq!(*player, P1, "\"that player may copy this spell\"");
                match copy {
                    // Chain of Smog's own offer: P1 copies it.
                    None => GameAction::DecideOptionalEffect { accept: true },
                    // The copy's offer, from inside the copy's own paused
                    // resolution: the copy's carrier owes the copy's final
                    // part, and Chain of Smog has already finished.
                    Some(copy) => {
                        assert_eq!(
                            state.owed_spell_final_part().map(|owed| owed.spell),
                            Some(copy),
                            "the copy's own paused resolution owes the copy's final part"
                        );
                        assert_eq!(
                            state.objects[&smog].zone,
                            Zone::Graveyard,
                            "reach guard: Chain of Smog has finished into its owner's graveyard"
                        );
                        copy_owed_at_its_offer = true;
                        GameAction::DecideOptionalEffect { accept: false }
                    }
                }
            }
            WaitingFor::CopyRetarget { copy_id, .. } => {
                let entry = state
                    .stack
                    .iter()
                    .find(|entry| entry.id == *copy_id)
                    .expect("the copy is on the stack");
                assert_ne!(entry.id, smog);
                assert_eq!(entry.controller, P1, "CR 707.10: P1 controls the copy");
                let StackEntryKind::Spell {
                    ability: Some(ability),
                    ..
                } = &entry.kind
                else {
                    panic!("the copy is a spell with an ability: {:?}", entry.kind);
                };
                assert_eq!(
                    ability.context, expected_copy_context,
                    "CR 707.10: the copy carries the announced context, and nothing of the \
                     original's paused resolution"
                );
                copy_context_checked = true;
                copy = Some(*copy_id);
                GameAction::ChooseTarget { target: None }
            }
            other => panic!("unexpected prompt while resolving: {other:?}"),
        };
        events.extend(
            runner
                .act(action)
                .expect("the engine accepts the answer")
                .events,
        );
    }

    assert!(
        smog_owed_at_its_discard,
        "reach guard: Chain of Smog paused"
    );
    assert!(copy_context_checked, "reach guard: P1 copied Chain of Smog");
    assert!(
        copy_owed_at_its_offer,
        "reach guard: the copy's own resolution paused on its offer"
    );
    let copy = copy.expect("the copy was made");
    let state = runner.state();
    assert!(
        state.players[P1.0 as usize].hand.is_empty(),
        "reach guard: the original made P1 discard two cards and the copy the last one"
    );
    assert_eq!(state.objects[&smog].zone, Zone::Graveyard);
    assert_eq!(stack_resolved_count(&events, smog), 1);
    assert_eq!(stack_resolved_count(&events, copy), 1);
    assert_settled(state);
}

/// CR 608.2m + CR 800.4a: Diabolic Edict ("Target player sacrifices a creature
/// of their choice.") pauses on its target's sacrifice. Its controller leaves
/// the game while that player is still answering: the spell is exiled with its
/// departing controller, but the resolution goes on for the player answering
/// its instruction, and the spell is reported resolved exactly once, when that
/// answer finishes it.
#[test]
fn a_spell_whose_controller_leaves_while_another_player_answers_is_reported_resolved_once() {
    let mut scenario = GameScenario::new_n_player(3, 42);
    scenario.at_phase(Phase::PreCombatMain);
    let edict = scenario.add_real_card(P0, "Diabolic Edict", Zone::Hand, db());
    let first = scenario.add_vanilla(P1, 2, 2);
    let second = scenario.add_vanilla(P1, 2, 2);
    scenario.with_mana_pool(P0, mana(ManaType::Black, 2));
    let mut runner = scenario.build();

    runner.cast(edict).target_player(P1).commit();
    pass_until_a_prompt(&mut runner);
    assert!(
        matches!(
            &runner.state().waiting_for,
            WaitingFor::EffectZoneChoice { player, cards, .. }
                if *player == P1 && cards.contains(&first) && cards.contains(&second)
        ),
        "reach guard: the Edict paused on P1's sacrifice, got {:?}",
        runner.state().waiting_for
    );

    let conceded = concede(&mut runner, P0);
    assert_eq!(stack_resolved_count(&conceded, edict), 0);
    assert!(
        matches!(
            runner.state().waiting_for,
            WaitingFor::EffectZoneChoice { player, .. } if player == P1
        ),
        "reach guard: CR 608.2m: P1 still answers the Edict, got {:?}",
        runner.state().waiting_for
    );
    assert_eq!(
        runner.state().objects[&edict].zone,
        Zone::Exile,
        "reach guard: CR 800.4a: the Edict left the game with its departing controller"
    );

    let answered = runner
        .act(GameAction::SelectCards { cards: vec![first] })
        .expect("P1 sacrifices a creature")
        .events;
    assert_eq!(
        stack_resolved_count(&answered, edict),
        1,
        "CR 608.2m: the resolution finished by P1 is reported exactly once"
    );
    let mut events = answered;
    events.extend(run_until_settled(&mut runner, |_| None));
    let state = runner.state();
    assert_eq!(state.objects[&first].zone, Zone::Graveyard);
    assert_eq!(state.objects[&edict].zone, Zone::Exile);
    assert_eq!(stack_resolved_count(&events, edict), 1);
    assert_settled(state);
}

/// CR 608.2n + CR 702.82a: Zombify ("Return target creature card from your
/// graveyard to the battlefield.") returning Mycoloth (Devour 2) pauses on the
/// devour choice inside Zombify's own resolution. The recovery guards that
/// refuse an owing carrier as terminal residue must not stall this live
/// resolution: once the devour choice is answered, Zombify's final part runs.
#[test]
fn zombify_returning_a_devourer_still_finishes_its_final_part() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let zombify = scenario.add_real_card(P0, "Zombify", Zone::Hand, db());
    let mycoloth = scenario.add_real_card(P0, "Mycoloth", Zone::Graveyard, db());
    scenario.add_vanilla(P0, 1, 1);
    scenario.add_vanilla(P0, 1, 1);
    scenario.with_mana_pool(P0, mana(ManaType::Black, 4));
    let mut runner = scenario.build();

    runner.cast(zombify).target_object(mycoloth).commit();
    let mut asked_to_devour = false;
    let events = run_until_settled(&mut runner, |state| {
        matches!(state.waiting_for, WaitingFor::EffectZoneChoice { .. }).then(|| {
            asked_to_devour = true;
            assert_eq!(
                state.objects[&zombify].zone,
                Zone::Stack,
                "Zombify is still resolving while the devour choice is asked"
            );
            GameAction::SelectCards { cards: vec![] }
        })
    });

    assert!(asked_to_devour, "reach guard: Mycoloth's devour was asked");
    let state = runner.state();
    assert_eq!(state.objects[&mycoloth].zone, Zone::Battlefield);
    assert_eq!(state.objects[&zombify].zone, Zone::Graveyard);
    assert_eq!(stack_resolved_count(&events, zombify), 1);
    assert_settled(state);
}
