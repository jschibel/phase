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
//! CR 733.1: a cancelled cast triggers nothing.
//!
//! https://github.com/phase-rs/phase/issues/9691

use engine::game::effects::attach::attach_to;
use engine::game::layers::evaluate_layers;
use engine::game::scenario::{GameRunner, GameScenario, P0, P1};
use engine::game::scenario_db::GameScenarioDbExt;
use engine::types::ability::TargetRef;
use engine::types::actions::GameAction;
use engine::types::game_state::{CastPaymentMode, StackEntryKind, WaitingFor};
use engine::types::identifiers::ObjectId;
use engine::types::mana::{ManaType, ManaUnit};
use engine::types::phase::Phase;
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

    runner
        .act(GameAction::CancelCast)
        .expect("cancel during payment");
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
