//! Regression for issue #9691: "becomes the target of a spell" triggers must
//! fire when the targeting spell's mana is paid interactively.
//!
//! CR 601.2c: abilities that trigger when an object becomes the target of a
//! spell trigger as the target is announced, and wait to be put on the stack
//! until the spell has finished being cast. CR 601.2i: the spell then becomes
//! cast. CR 603.3: the waiting abilities are put on the stack the next time a
//! player would receive priority. The cast's payment mode cannot matter: with
//! `CastPaymentMode::Manual` the announcing action pauses at `ManaPayment`, and
//! the trigger must still be waiting when the payment completes.
//!
//! CR 603.3b: the held abilities are one batch with the abilities that trigger
//! as the spell becomes cast (prowess, "whenever you cast"), so a paused cast
//! must put the same triggers on the stack, in the same APNAP order and behind
//! the same ordering prompt, as a cast that never paused.
//!
//! CR 733.1: a cancelled cast triggers nothing. CR 800.4a: a player who leaves
//! the game mid-cast takes their held triggers with them.
//!
//! https://github.com/phase-rs/phase/issues/9691

use engine::game::effects::attach::attach_to;
use engine::game::layers::evaluate_layers;
use engine::game::scenario::{GameRunner, GameScenario, P0, P1};
use engine::game::scenario_db::GameScenarioDbExt;
use engine::types::ability::TargetRef;
use engine::types::actions::GameAction;
use engine::types::game_state::{CastPaymentMode, GameState, StackEntryKind, WaitingFor};
use engine::types::identifiers::ObjectId;
use engine::types::mana::{ManaType, ManaUnit};
use engine::types::phase::Phase;
use engine::types::player::PlayerId;
use engine::types::zones::Zone;

use super::support::shared_card_db;

fn give_red(runner: &mut GameRunner) {
    runner.state_mut().players[P0.0 as usize]
        .mana_pool
        .add(ManaUnit::new(ManaType::Red, ObjectId(0), false, vec![]));
}

/// Cast P0's Lightning Bolt at `target`, announcing the target through the
/// target-selection prompt and paying in `mode`. Returns once the cast is
/// complete and a player has priority.
fn cast_bolt_at(runner: &mut GameRunner, bolt: ObjectId, target: ObjectId, mode: CastPaymentMode) {
    let card_id = runner.state().objects[&bolt].card_id;
    runner
        .act(GameAction::CastSpell {
            object_id: bolt,
            card_id,
            targets: vec![],
            payment_mode: mode,
        })
        .expect("announce Lightning Bolt");
    assert!(
        matches!(
            runner.state().waiting_for,
            WaitingFor::TargetSelection { .. }
        ),
        "Lightning Bolt asks for its target, got {:?}",
        runner.state().waiting_for
    );
    runner
        .act(GameAction::SelectTargets {
            targets: vec![TargetRef::Object(target)],
        })
        .expect("announce Lightning Bolt's target");

    if mode == CastPaymentMode::Manual {
        assert!(
            matches!(runner.state().waiting_for, WaitingFor::ManaPayment { .. }),
            "manual payment pauses the cast after its target is announced, got {:?}",
            runner.state().waiting_for
        );
        assert_eq!(
            runner.state().stack.len(),
            1,
            "nothing but the spell being cast is on the stack while it is paid for"
        );
        runner
            .act(GameAction::PassPriority)
            .expect("pay Lightning Bolt from the pool");
    }
    assert!(
        matches!(runner.state().waiting_for, WaitingFor::Priority { .. }),
        "the cast is complete, got {:?}",
        runner.state().waiting_for
    );
    assert_eq!(runner.state().objects[&bolt].zone, Zone::Stack);
}

/// Whether the game holds becomes-target triggers for a spell still being
/// cast. The carrier is engine-private, so it is read from the serialized
/// state, which omits it when empty.
fn holds_spell_target_triggers(state: &GameState) -> bool {
    serde_json::to_value(state)
        .expect("serialize state")
        .get("announced_spell_trigger_collections")
        .is_some()
}

/// The source of the triggered ability on top of the stack, if the top entry
/// is a triggered ability.
fn top_trigger_source(runner: &GameRunner) -> Option<ObjectId> {
    let top = runner.state().stack.last()?;
    matches!(top.kind, StackEntryKind::TriggeredAbility { .. }).then_some(top.source_id)
}

fn bonecrusher_scenario() -> (GameRunner, ObjectId, ObjectId) {
    let db = shared_card_db().expect("card db");
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let bolt = scenario.add_real_card(P0, "Lightning Bolt", Zone::Hand, db);
    let giant = scenario.add_real_card(P1, "Bonecrusher Giant", Zone::Battlefield, db);
    let mut runner = scenario.build();
    engine::game::rehydrate_game_from_card_db(runner.state_mut(), db);
    give_red(&mut runner);
    (runner, bolt, giant)
}

/// Bonecrusher Giant: "Whenever this creature becomes the target of a spell,
/// this creature deals 2 damage to that spell's controller."
fn bonecrusher_punishes_the_bolt(mode: CastPaymentMode) {
    let (mut runner, bolt, giant) = bonecrusher_scenario();

    cast_bolt_at(&mut runner, bolt, giant, mode);

    assert_eq!(
        runner.state().stack.len(),
        2,
        "{mode:?}: Bonecrusher Giant's trigger waits above the cast Bolt"
    );
    assert_eq!(top_trigger_source(&runner), Some(giant));

    runner.advance_until_stack_empty();
    assert_eq!(
        runner.life(P0),
        18,
        "{mode:?}: the Giant deals 2 damage to the Bolt's controller"
    );
    assert_eq!(
        runner.state().objects[&giant].zone,
        Zone::Graveyard,
        "{mode:?}: the Bolt still resolves and kills the Giant"
    );
}

#[test]
fn bonecrusher_giant_triggers_when_the_bolt_is_paid_manually() {
    bonecrusher_punishes_the_bolt(CastPaymentMode::Manual);
}

#[test]
fn bonecrusher_giant_triggers_when_the_bolt_is_paid_automatically() {
    bonecrusher_punishes_the_bolt(CastPaymentMode::Auto);
}

/// CR 733.1: no abilities trigger as a result of an undone action. A Bolt
/// cancelled during its payment leaves no trigger behind — not on the stack,
/// and not waiting for the next cast of the same card.
#[test]
fn a_cancelled_bolt_leaves_no_bonecrusher_trigger_behind() {
    let (mut runner, bolt, giant) = bonecrusher_scenario();
    let card_id = runner.state().objects[&bolt].card_id;
    runner
        .act(GameAction::CastSpell {
            object_id: bolt,
            card_id,
            targets: vec![],
            payment_mode: CastPaymentMode::Manual,
        })
        .expect("announce Lightning Bolt");
    runner
        .act(GameAction::SelectTargets {
            targets: vec![TargetRef::Object(giant)],
        })
        .expect("announce Lightning Bolt's target");
    assert!(matches!(
        runner.state().waiting_for,
        WaitingFor::ManaPayment { .. }
    ));
    assert!(
        holds_spell_target_triggers(runner.state()),
        "the paused cast holds the Giant's trigger"
    );

    runner
        .act(GameAction::CancelCast)
        .expect("cancel during payment");
    assert!(
        !holds_spell_target_triggers(runner.state()),
        "the cancelling action itself drops the held trigger"
    );
    assert_eq!(runner.state().objects[&bolt].zone, Zone::Hand);
    assert!(
        runner.state().stack.is_empty(),
        "a cancelled cast puts no trigger on the stack"
    );
    assert!(runner.state().deferred_triggers.is_empty());
    assert_eq!(runner.life(P0), 20);

    // Casting the same card again triggers the Giant once, for this cast only.
    cast_bolt_at(&mut runner, bolt, giant, CastPaymentMode::Manual);
    assert_eq!(
        runner.state().stack.len(),
        2,
        "only the completed cast's trigger waits on the stack"
    );
    runner.advance_until_stack_empty();
    assert_eq!(runner.life(P0), 18);
}

/// Phantasmal Image's copy has "When this creature becomes the target of a
/// spell or ability, sacrifice it." The sacrifice resolves first, so the Bolt
/// has no legal target when it resolves.
fn phantasmal_image_is_sacrificed(mode: CastPaymentMode) {
    let db = shared_card_db().expect("card db");
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let bolt = scenario.add_real_card(P0, "Lightning Bolt", Zone::Hand, db);
    let image = scenario.add_real_card(P0, "Phantasmal Image", Zone::Hand, db);
    let bears = scenario.add_real_card(P1, "Grizzly Bears", Zone::Battlefield, db);
    let mut runner = scenario.build();
    engine::game::rehydrate_game_from_card_db(runner.state_mut(), db);
    runner.state_mut().players[P0.0 as usize]
        .mana_pool
        .add(ManaUnit::new(ManaType::Blue, ObjectId(0), false, vec![]));
    runner.state_mut().players[P0.0 as usize]
        .mana_pool
        .add(ManaUnit::new(
            ManaType::Colorless,
            ObjectId(0),
            false,
            vec![],
        ));

    let card_id = runner.state().objects[&image].card_id;
    runner
        .act(GameAction::CastSpell {
            object_id: image,
            card_id,
            targets: vec![],
            payment_mode: CastPaymentMode::Auto,
        })
        .expect("cast Phantasmal Image");
    for _ in 0..32 {
        match runner.state().waiting_for.clone() {
            WaitingFor::ReplacementChoice { .. } => {
                runner
                    .act(GameAction::ChooseReplacement { index: 0 })
                    .expect("enter as a copy");
            }
            WaitingFor::CopyTargetChoice { .. } => {
                runner
                    .act(GameAction::ChooseTarget {
                        target: Some(TargetRef::Object(bears)),
                    })
                    .expect("copy Grizzly Bears");
            }
            WaitingFor::OptionalEffectChoice { .. } => {
                runner
                    .act(GameAction::DecideOptionalEffect { accept: true })
                    .expect("accept the copy");
            }
            WaitingFor::Priority { .. } if runner.state().stack.is_empty() => break,
            WaitingFor::Priority { .. } => {
                runner.act(GameAction::PassPriority).expect("pass");
            }
            other => panic!("unexpected prompt while Phantasmal Image enters: {other:?}"),
        }
    }
    assert_eq!(runner.state().objects[&image].zone, Zone::Battlefield);
    assert_eq!(
        runner.state().objects[&image].name,
        "Grizzly Bears",
        "reach guard: the Image entered as a copy"
    );

    give_red(&mut runner);
    cast_bolt_at(&mut runner, bolt, image, mode);
    assert_eq!(
        top_trigger_source(&runner),
        Some(image),
        "{mode:?}: the Image's sacrifice trigger waits above the cast Bolt"
    );

    runner.advance_until_stack_empty();
    assert_eq!(
        runner.state().objects[&image].zone,
        Zone::Graveyard,
        "{mode:?}: the Image is sacrificed"
    );
}

#[test]
fn phantasmal_image_is_sacrificed_when_a_manually_paid_bolt_targets_it() {
    phantasmal_image_is_sacrificed(CastPaymentMode::Manual);
}

#[test]
fn phantasmal_image_is_sacrificed_when_an_automatically_paid_bolt_targets_it() {
    phantasmal_image_is_sacrificed(CastPaymentMode::Auto);
}

/// CR 702.21a: Ward is a triggered ability — "Whenever this permanent becomes
/// the target of a spell or ability an opponent controls, counter that spell or
/// ability unless that player pays [cost]." Lavaspur Boots grants ward {1}; the
/// Bolt's controller has no mana left, so the Bolt is countered.
fn granted_ward_counters_the_bolt(mode: CastPaymentMode) {
    let db = shared_card_db().expect("card db");
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let bolt = scenario.add_real_card(P0, "Lightning Bolt", Zone::Hand, db);
    let bears = scenario.add_real_card(P1, "Grizzly Bears", Zone::Battlefield, db);
    let boots = scenario.add_real_card(P1, "Lavaspur Boots", Zone::Battlefield, db);
    let mut runner = scenario.build();
    engine::game::rehydrate_game_from_card_db(runner.state_mut(), db);
    attach_to(runner.state_mut(), boots, bears);
    evaluate_layers(runner.state_mut());
    assert!(
        runner.state().objects[&bears].attachments.contains(&boots),
        "reach guard: the Boots equip the Bears"
    );
    give_red(&mut runner);

    cast_bolt_at(&mut runner, bolt, bears, mode);
    assert_eq!(
        runner.state().stack.len(),
        2,
        "{mode:?}: the granted ward trigger waits above the cast Bolt"
    );
    assert_eq!(top_trigger_source(&runner), Some(bears));

    for _ in 0..16 {
        match runner.state().waiting_for {
            WaitingFor::UnlessPayment { .. } => {
                runner
                    .act(GameAction::PayUnlessCost { pay: false })
                    .expect("decline the ward cost");
            }
            WaitingFor::Priority { .. } if runner.state().stack.is_empty() => break,
            _ => runner.advance_until_stack_empty(),
        }
    }
    assert!(runner.state().stack.is_empty());
    assert_eq!(
        runner.state().objects[&bolt].zone,
        Zone::Graveyard,
        "{mode:?}: the Bolt is countered"
    );
    assert_eq!(
        runner.state().objects[&bears].zone,
        Zone::Battlefield,
        "{mode:?}: ward protected the Bears"
    );
    assert_eq!(runner.state().objects[&bears].damage_marked, 0);
}

#[test]
fn granted_ward_triggers_when_a_manually_paid_bolt_targets_its_creature() {
    granted_ward_counters_the_bolt(CastPaymentMode::Manual);
}

#[test]
fn granted_ward_triggers_when_an_automatically_paid_bolt_targets_its_creature() {
    granted_ward_counters_the_bolt(CastPaymentMode::Auto);
}

// ---------------------------------------------------------------------------
// CR 603.3b: one batch with the spell's cast triggers, in either payment mode.
// ---------------------------------------------------------------------------

fn add_mana(runner: &mut GameRunner, player: PlayerId, mana_type: ManaType, n: usize) {
    for _ in 0..n {
        runner.state_mut().players[player.0 as usize]
            .mana_pool
            .add(ManaUnit::new(mana_type, ObjectId(0), false, vec![]));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Entry {
    Spell(ObjectId),
    Trigger(ObjectId),
}

/// What a completed cast leaves behind: the ordering prompt it raised, if any
/// (its player and how many triggers it orders), and the stack, bottom first.
#[derive(Debug, PartialEq)]
struct Outcome {
    ordering: Option<(PlayerId, usize)>,
    stack: Vec<Entry>,
}

fn outcome(runner: &GameRunner) -> Outcome {
    let state = runner.state();
    let ordering = match &state.waiting_for {
        WaitingFor::OrderTriggers { player, triggers } => Some((*player, triggers.len())),
        WaitingFor::Priority { .. } => None,
        other => panic!("the cast is complete, got {other:?}"),
    };
    let stack = state
        .stack
        .iter()
        .map(|entry| match entry.kind {
            StackEntryKind::Spell { .. } => Entry::Spell(entry.source_id),
            StackEntryKind::TriggeredAbility { .. } => Entry::Trigger(entry.source_id),
            ref other => panic!("unexpected stack entry {other:?}"),
        })
        .collect();
    Outcome { ordering, stack }
}

/// The casting prompts a cast passed through, by kind.
type Prompts = Vec<&'static str>;

/// Cast `spell` at `targets`, answering every casting prompt: the targets are
/// announced through the prompt, a division gives each target an equal share
/// (the remainder to the first), a cost selection takes `sacrifice`, and a
/// manual payment is paid from the pool. Returns the prompts passed through.
fn cast(
    runner: &mut GameRunner,
    spell: ObjectId,
    targets: &[ObjectId],
    sacrifice: Option<ObjectId>,
    mode: CastPaymentMode,
) -> Prompts {
    let card_id = runner.state().objects[&spell].card_id;
    runner
        .act(GameAction::CastSpell {
            object_id: spell,
            card_id,
            targets: vec![],
            payment_mode: mode,
        })
        .expect("announce the spell");
    let mut prompts = Vec::new();
    for _ in 0..16 {
        let action = match runner.state().waiting_for.clone() {
            WaitingFor::TargetSelection { .. } => GameAction::SelectTargets {
                targets: targets.iter().copied().map(TargetRef::Object).collect(),
            },
            WaitingFor::DistributeAmong { total, targets, .. } => {
                let share = total / targets.len() as u32;
                let mut distribution: Vec<(TargetRef, u32)> =
                    targets.into_iter().map(|target| (target, share)).collect();
                distribution[0].1 += total - share * distribution.len() as u32;
                GameAction::DistributeAmong { distribution }
            }
            WaitingFor::PayCost { .. } => GameAction::SelectCards {
                cards: vec![sacrifice.expect("the cast asks for a cost selection")],
            },
            WaitingFor::ManaPayment { .. } => GameAction::PassPriority,
            _ => return prompts,
        };
        prompts.push(runner.waiting_for_kind());
        runner.act(action).expect("answer the casting prompt");
    }
    panic!("the cast never completed: {prompts:?}");
}

/// Build the same position for each payment mode, cast in it, and require the
/// two casts to end identically. Returns the prompts of the manual cast and the
/// shared outcome.
fn same_in_both_modes(
    mut build_and_cast: impl FnMut(CastPaymentMode) -> (Prompts, Outcome),
) -> (Prompts, Outcome) {
    let (auto_prompts, auto) = build_and_cast(CastPaymentMode::Auto);
    let (manual_prompts, manual) = build_and_cast(CastPaymentMode::Manual);
    assert!(
        !auto_prompts.contains(&"ManaPayment"),
        "reach guard: the automatic cast pays without a payment prompt: {auto_prompts:?}"
    );
    assert_eq!(
        manual_prompts.last(),
        Some(&"ManaPayment"),
        "reach guard: the manual cast pauses for payment after its other choices"
    );
    assert_eq!(
        manual, auto,
        "a cast paused for payment leaves the triggers, order and prompt of one that was not"
    );
    (manual_prompts, auto)
}

/// The ids of one position, identical in every build of it.
#[derive(Clone, Copy, Debug, Default)]
struct Ids {
    spell: Option<ObjectId>,
    a: Option<ObjectId>,
    b: Option<ObjectId>,
}

/// A Lightning Bolt in P0's hand, the board `build` adds (returning the Bolt's
/// target and up to two ids worth naming), and red mana to cast the Bolt.
fn cast_bolt_on_board(
    mode: CastPaymentMode,
    ids: &mut Ids,
    build: impl FnOnce(&mut GameScenario, &mut Ids) -> ObjectId,
    prepare: impl FnOnce(&mut GameRunner, &Ids),
) -> (Prompts, Outcome) {
    let db = shared_card_db().expect("card db");
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let bolt = scenario.add_real_card(P0, "Lightning Bolt", Zone::Hand, db);
    ids.spell = Some(bolt);
    let target = build(&mut scenario, ids);
    let mut runner = scenario.build();
    engine::game::rehydrate_game_from_card_db(runner.state_mut(), db);
    prepare(&mut runner, ids);
    add_mana(&mut runner, P0, ManaType::Red, 1);
    let prompts = cast(&mut runner, bolt, &[target], None, mode);
    (prompts, outcome(&runner))
}

/// Monastery Swiftspear has prowess ("Whenever you cast a noncreature spell");
/// Bonecrusher Giant triggers on becoming the target of a spell. With both
/// under the caster's control, CR 603.3b lets the caster put them on the stack
/// in any order, so the cast asks — whether or not it paused for payment.
#[test]
fn a_casters_prowess_and_bonecrusher_trigger_are_ordered_together_in_either_mode() {
    let mut ids = Ids::default();
    let (_, outcome) = same_in_both_modes(|mode| {
        cast_bolt_on_board(
            mode,
            &mut ids,
            |scenario, ids| {
                let db = shared_card_db().expect("card db");
                ids.a =
                    Some(scenario.add_real_card(P0, "Monastery Swiftspear", Zone::Battlefield, db));
                scenario.add_real_card(P0, "Bonecrusher Giant", Zone::Battlefield, db)
            },
            |_, _| {},
        )
    });
    assert_eq!(
        outcome.ordering,
        Some((P0, 2)),
        "the caster orders the prowess and Giant triggers as one batch"
    );
    assert_eq!(outcome.stack, vec![Entry::Spell(ids.spell.unwrap())]);
}

/// CR 603.3b: the active player's triggers go on the stack first, then the
/// non-active player's — the opponent's Giant trigger above the caster's
/// prowess trigger, even though the Giant's was held through a payment.
#[test]
fn an_opponents_bonecrusher_trigger_goes_above_the_casters_prowess_in_either_mode() {
    let mut ids = Ids::default();
    let (_, outcome) = same_in_both_modes(|mode| {
        cast_bolt_on_board(
            mode,
            &mut ids,
            |scenario, ids| {
                let db = shared_card_db().expect("card db");
                ids.a =
                    Some(scenario.add_real_card(P0, "Monastery Swiftspear", Zone::Battlefield, db));
                let giant = scenario.add_real_card(P1, "Bonecrusher Giant", Zone::Battlefield, db);
                ids.b = Some(giant);
                giant
            },
            |_, _| {},
        )
    });
    assert_eq!(outcome.ordering, None);
    assert_eq!(
        outcome.stack,
        vec![
            Entry::Spell(ids.spell.unwrap()),
            Entry::Trigger(ids.a.unwrap()),
            Entry::Trigger(ids.b.unwrap()),
        ],
        "AP's prowess trigger first, NAP's Giant trigger on top"
    );
}

/// The mirror: the caster's own Giant is targeted while the opponent's Esper
/// Sentinel triggers on the cast ("Whenever an opponent casts their first
/// noncreature spell each turn"). The opponent's trigger goes on top.
#[test]
fn an_opponents_cast_trigger_goes_above_the_casters_bonecrusher_in_either_mode() {
    let mut ids = Ids::default();
    let (_, outcome) = same_in_both_modes(|mode| {
        cast_bolt_on_board(
            mode,
            &mut ids,
            |scenario, ids| {
                let db = shared_card_db().expect("card db");
                let giant = scenario.add_real_card(P0, "Bonecrusher Giant", Zone::Battlefield, db);
                ids.a = Some(giant);
                ids.b = Some(scenario.add_real_card(P1, "Esper Sentinel", Zone::Battlefield, db));
                giant
            },
            |_, _| {},
        )
    });
    assert_eq!(outcome.ordering, None);
    assert_eq!(
        outcome.stack,
        vec![
            Entry::Spell(ids.spell.unwrap()),
            Entry::Trigger(ids.a.unwrap()),
            Entry::Trigger(ids.b.unwrap()),
        ],
        "AP's Giant trigger first, NAP's Sentinel trigger on top"
    );
}

/// Lavaspur Boots grant ward {1} to the Giant, which has its own trigger too.
/// Both trigger on one Bolt, and both are the Giant's controller's to order.
#[test]
fn ward_and_bonecrusher_on_one_target_are_ordered_together_in_either_mode() {
    let mut ids = Ids::default();
    let (_, outcome) = same_in_both_modes(|mode| {
        cast_bolt_on_board(
            mode,
            &mut ids,
            |scenario, ids| {
                let db = shared_card_db().expect("card db");
                let giant = scenario.add_real_card(P1, "Bonecrusher Giant", Zone::Battlefield, db);
                ids.a = Some(giant);
                ids.b = Some(scenario.add_real_card(P1, "Lavaspur Boots", Zone::Battlefield, db));
                giant
            },
            |runner, ids| {
                attach_to(runner.state_mut(), ids.b.unwrap(), ids.a.unwrap());
                evaluate_layers(runner.state_mut());
            },
        )
    });
    assert_eq!(
        outcome.ordering,
        Some((P1, 2)),
        "the Giant's controller orders its ward and its own trigger"
    );
}

/// CR 601.2d: Forked Bolt pauses after its targets for the division of its
/// damage — a pause in either payment mode. Each targeted Giant triggers once,
/// and both triggers are their controller's to order.
#[test]
fn a_divided_bolt_triggers_each_targeted_giant_once_in_either_mode() {
    let (prompts, outcome) = same_in_both_modes(|mode| {
        let db = shared_card_db().expect("card db");
        let mut scenario = GameScenario::new();
        scenario.at_phase(Phase::PreCombatMain);
        let forked = scenario.add_real_card(P0, "Forked Bolt", Zone::Hand, db);
        let first = scenario.add_real_card(P1, "Bonecrusher Giant", Zone::Battlefield, db);
        let second = scenario.add_real_card(P1, "Bonecrusher Giant", Zone::Battlefield, db);
        let mut runner = scenario.build();
        engine::game::rehydrate_game_from_card_db(runner.state_mut(), db);
        add_mana(&mut runner, P0, ManaType::Red, 1);
        let prompts = cast(&mut runner, forked, &[first, second], None, mode);
        (prompts, outcome(&runner))
    });
    assert_eq!(
        prompts,
        vec!["TargetSelection", "DistributeAmong", "ManaPayment"],
        "reach guard: the division is announced between the targets and the payment"
    );
    assert_eq!(outcome.ordering, Some((P1, 2)));
}

/// CR 601.2h: Bone Splinters' additional cost ("As an additional cost to cast
/// this spell, sacrifice a creature") is chosen after its target is announced —
/// a pause in either payment mode. The opponent's Giant trigger and the caster's
/// prowess trigger still form one APNAP-ordered batch.
#[test]
fn a_sacrifice_cost_pause_keeps_one_apnap_batch_in_either_mode() {
    let mut ids = Ids::default();
    let (prompts, outcome) = same_in_both_modes(|mode| {
        let db = shared_card_db().expect("card db");
        let mut scenario = GameScenario::new();
        scenario.at_phase(Phase::PreCombatMain);
        let splinters = scenario.add_real_card(P0, "Bone Splinters", Zone::Hand, db);
        let spear = scenario.add_real_card(P0, "Monastery Swiftspear", Zone::Battlefield, db);
        let bears = scenario.add_real_card(P0, "Grizzly Bears", Zone::Battlefield, db);
        let giant = scenario.add_real_card(P1, "Bonecrusher Giant", Zone::Battlefield, db);
        ids = Ids {
            spell: Some(splinters),
            a: Some(spear),
            b: Some(giant),
        };
        let mut runner = scenario.build();
        engine::game::rehydrate_game_from_card_db(runner.state_mut(), db);
        add_mana(&mut runner, P0, ManaType::Black, 1);
        let prompts = cast(&mut runner, splinters, &[giant], Some(bears), mode);
        assert_eq!(
            runner.state().objects[&bears].zone,
            Zone::Graveyard,
            "{mode:?}: the Bears were sacrificed to cast Bone Splinters"
        );
        (prompts, outcome(&runner))
    });
    assert_eq!(
        prompts,
        vec!["TargetSelection", "PayCost", "ManaPayment"],
        "reach guard: the sacrifice is chosen between the target and the payment"
    );
    assert_eq!(outcome.ordering, None);
    assert_eq!(
        outcome.stack,
        vec![
            Entry::Spell(ids.spell.unwrap()),
            Entry::Trigger(ids.a.unwrap()),
            Entry::Trigger(ids.b.unwrap()),
        ],
        "AP's prowess trigger first, NAP's Giant trigger on top"
    );
}

/// Kira, Great Glass-Spinner: "Creatures you control have 'Whenever this
/// creature becomes the target of a spell or ability for the first time each
/// turn, counter that spell or ability.'" A held trigger spends the
/// first-time-each-turn allowance exactly once, as an unpaused one does, so a
/// second Bolt the same turn is not countered — in either mode.
#[test]
fn kiras_first_time_each_turn_is_spent_once_in_either_mode() {
    same_in_both_modes(|mode| {
        let db = shared_card_db().expect("card db");
        let mut scenario = GameScenario::new();
        scenario.at_phase(Phase::PreCombatMain);
        let first = scenario.add_real_card(P0, "Lightning Bolt", Zone::Hand, db);
        let second = scenario.add_real_card(P0, "Lightning Bolt", Zone::Hand, db);
        let spellskite = scenario.add_real_card(P1, "Spellskite", Zone::Battlefield, db);
        scenario.add_real_card(P1, "Kira, Great Glass-Spinner", Zone::Battlefield, db);
        let mut runner = scenario.build();
        engine::game::rehydrate_game_from_card_db(runner.state_mut(), db);
        add_mana(&mut runner, P0, ManaType::Red, 2);

        cast(&mut runner, first, &[spellskite], None, mode);
        assert_eq!(
            outcome(&runner).stack,
            vec![Entry::Spell(first), Entry::Trigger(spellskite)],
            "{mode:?}: the first Bolt triggers Kira's granted ability"
        );
        runner.advance_until_stack_empty();
        assert_eq!(runner.state().objects[&first].zone, Zone::Graveyard);

        let prompts = cast(&mut runner, second, &[spellskite], None, mode);
        (prompts, outcome(&runner))
    });
}

/// CR 733.1: no abilities trigger as a result of an undone action, so a
/// cancelled cast spends no first-time-each-turn allowance; the recast
/// triggers Kira's granted ability.
#[test]
fn a_cancelled_cast_spends_no_first_time_each_turn_allowance() {
    let db = shared_card_db().expect("card db");
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let bolt = scenario.add_real_card(P0, "Lightning Bolt", Zone::Hand, db);
    let spellskite = scenario.add_real_card(P1, "Spellskite", Zone::Battlefield, db);
    scenario.add_real_card(P1, "Kira, Great Glass-Spinner", Zone::Battlefield, db);
    let mut runner = scenario.build();
    engine::game::rehydrate_game_from_card_db(runner.state_mut(), db);
    add_mana(&mut runner, P0, ManaType::Red, 1);

    let card_id = runner.state().objects[&bolt].card_id;
    runner
        .act(GameAction::CastSpell {
            object_id: bolt,
            card_id,
            targets: vec![],
            payment_mode: CastPaymentMode::Manual,
        })
        .expect("announce Lightning Bolt");
    runner
        .act(GameAction::SelectTargets {
            targets: vec![TargetRef::Object(spellskite)],
        })
        .expect("announce Lightning Bolt's target");
    assert!(matches!(
        runner.state().waiting_for,
        WaitingFor::ManaPayment { .. }
    ));
    runner
        .act(GameAction::CancelCast)
        .expect("cancel during payment");

    cast(
        &mut runner,
        bolt,
        &[spellskite],
        None,
        CastPaymentMode::Manual,
    );
    assert_eq!(
        outcome(&runner).stack,
        vec![Entry::Spell(bolt), Entry::Trigger(spellskite)],
        "the completed cast is the first time this turn"
    );
}

/// CR 800.4a: a player who leaves the game takes the abilities they control
/// with them. Forked Bolt targets a Giant of each opponent; the first
/// opponent concedes while the Bolt is being paid for. Only the remaining
/// opponent's Giant trigger reaches the stack.
#[test]
fn a_player_who_concedes_mid_cast_takes_their_held_trigger_with_them() {
    let p2 = PlayerId(2);
    let db = shared_card_db().expect("card db");
    let mut scenario = GameScenario::new_n_player(3, 42);
    scenario.at_phase(Phase::PreCombatMain);
    let forked = scenario.add_real_card(P0, "Forked Bolt", Zone::Hand, db);
    let departing = scenario.add_real_card(P1, "Bonecrusher Giant", Zone::Battlefield, db);
    let staying = scenario.add_real_card(p2, "Bonecrusher Giant", Zone::Battlefield, db);
    let mut runner = scenario.build();
    engine::game::rehydrate_game_from_card_db(runner.state_mut(), db);
    add_mana(&mut runner, P0, ManaType::Red, 1);

    let card_id = runner.state().objects[&forked].card_id;
    runner
        .act(GameAction::CastSpell {
            object_id: forked,
            card_id,
            targets: vec![],
            payment_mode: CastPaymentMode::Manual,
        })
        .expect("announce Forked Bolt");
    runner
        .act(GameAction::SelectTargets {
            targets: vec![TargetRef::Object(departing), TargetRef::Object(staying)],
        })
        .expect("announce Forked Bolt's targets");
    runner
        .act(GameAction::DistributeAmong {
            distribution: vec![
                (TargetRef::Object(departing), 1),
                (TargetRef::Object(staying), 1),
            ],
        })
        .expect("divide Forked Bolt's damage");
    assert!(matches!(
        runner.state().waiting_for,
        WaitingFor::ManaPayment { .. }
    ));

    runner
        .act(GameAction::Concede { player_id: P1 })
        .expect("P1 concedes while Forked Bolt is paid for");
    assert!(runner.state().players[P1.0 as usize].is_eliminated);
    assert!(
        matches!(runner.state().waiting_for, WaitingFor::ManaPayment { .. }),
        "the cast continues, got {:?}",
        runner.state().waiting_for
    );
    runner
        .act(GameAction::PassPriority)
        .expect("pay Forked Bolt from the pool");

    assert_eq!(
        outcome(&runner).stack,
        vec![Entry::Spell(forked), Entry::Trigger(staying)],
        "only the remaining opponent's Giant trigger waits above the Bolt"
    );
}

/// The held trigger is part of the saved game: a game saved while the Bolt is
/// being paid for, and loaded again, still puts the Giant's trigger on the
/// stack when the payment completes.
#[test]
fn a_held_trigger_survives_saving_and_loading_mid_cast() {
    let (mut runner, bolt, giant) = bonecrusher_scenario();
    let card_id = runner.state().objects[&bolt].card_id;
    runner
        .act(GameAction::CastSpell {
            object_id: bolt,
            card_id,
            targets: vec![],
            payment_mode: CastPaymentMode::Manual,
        })
        .expect("announce Lightning Bolt");
    runner
        .act(GameAction::SelectTargets {
            targets: vec![TargetRef::Object(giant)],
        })
        .expect("announce Lightning Bolt's target");
    assert!(holds_spell_target_triggers(runner.state()));

    let saved = serde_json::to_string(runner.state()).expect("save the game");
    *runner.state_mut() = serde_json::from_str(&saved).expect("load the game");
    engine::game::rehydrate_game_from_card_db(
        runner.state_mut(),
        shared_card_db().expect("card db"),
    );
    runner
        .act(GameAction::PassPriority)
        .expect("pay Lightning Bolt from the pool");
    assert_eq!(
        outcome(&runner).stack,
        vec![Entry::Spell(bolt), Entry::Trigger(giant)]
    );
}

/// The held trigger is engine-private, like an announced activation's: no
/// player's view of the game, and no spectator's, carries it.
#[test]
fn a_held_trigger_is_in_no_players_view() {
    let (mut runner, bolt, giant) = bonecrusher_scenario();
    let card_id = runner.state().objects[&bolt].card_id;
    runner
        .act(GameAction::CastSpell {
            object_id: bolt,
            card_id,
            targets: vec![],
            payment_mode: CastPaymentMode::Manual,
        })
        .expect("announce Lightning Bolt");
    runner
        .act(GameAction::SelectTargets {
            targets: vec![TargetRef::Object(giant)],
        })
        .expect("announce Lightning Bolt's target");
    assert!(holds_spell_target_triggers(runner.state()));

    use engine::game::visibility::{filter_state_for_unseated_viewer, filter_state_for_viewer};
    for view in [
        filter_state_for_viewer(runner.state(), P0),
        filter_state_for_viewer(runner.state(), P1),
        filter_state_for_unseated_viewer(runner.state()),
    ] {
        assert!(!holds_spell_target_triggers(&view));
    }
}
