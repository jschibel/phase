//! Grist, the Hunger Tide: a zone-declared self static functions in the zones it
//! declares.
//!
//! Oracle: "As long as Grist isn't on the battlefield, it's a 1/1 Insect creature
//! in addition to its other types." CR 113.6c: an ability that states which zones
//! it doesn't function in functions everywhere except for the specified zones.
//! Rulings: anywhere but on the battlefield, Grist is a Legendary Planeswalker
//! Creature; opponents can't make you discard her with Duress; a Grist in your
//! graveyard is a creature card and counts for her own -5.
//!
//! Every row drives a production entry (`GameRunner::cast`, `activate`, `act`)
//! with the verbatim Oracle text. A milled Grist is checked as a card that has just
//! reached the graveyard (row I9), not through the +1.

use crate::support::shared_card_db;
use engine::game::layers::flush_layers;
use engine::game::printed_cards::{
    rehydrate_game_from_card_db_with_finalization, CardDbRehydrationFinalization,
};
use engine::game::scenario::{GameRunner, GameScenario, P0, P1};
use engine::game::scenario_db::GameScenarioDbExt;
use engine::game::zones::move_to_zone;
use engine::types::ability::{
    ChosenAttribute, Effect, ManaSpendRestriction, ResolvedAbility, StaticCondition,
    StaticDefinition, TargetFilter, TargetRef,
};
use engine::types::actions::GameAction;
use engine::types::card_type::{CoreType, Supertype};
use engine::types::counter::CounterType;
use engine::types::events::GameEvent;
use engine::types::game_state::{
    CastingVariant, GameState, LayersDirty, ManaChoice, ManaChoicePrompt, PersistedGameState,
    PersistedRestoreFinalization, StackEntryKind, WaitingFor,
};
use engine::types::identifiers::{CardId, ObjectId, ObjectIncarnationRef};
use engine::types::mana::{
    ManaColor, ManaCost, ManaCostShard, ManaRestriction, ManaSourceOutput, ManaSpellGrant,
    ManaType, ManaUnit, SpellMeta,
};
use engine::types::phase::Phase;
use engine::types::player::PlayerId;
use engine::types::statics::StaticMode;
use engine::types::zones::Zone;
use std::sync::Arc;

const GRIST_ORACLE: &str = "As long as Grist isn't on the battlefield, it's a 1/1 Insect creature in addition to its other types.\n+1: Create a 1/1 black and green Insect creature token, then mill a card. If an Insect card was milled this way, put a loyalty counter on Grist and repeat this process.\n\u{2212}2: You may sacrifice a creature. When you do, destroy target creature or planeswalker.\n\u{2212}5: Each opponent loses life equal to the number of creature cards in your graveyard.";
const DURESS: &str = "Target opponent reveals their hand. You choose a noncreature, nonland card from it. That player discards that card.";
const GREEN_SUNS_ZENITH: &str = "Search your library for a green creature card with mana value X or less, put it onto the battlefield, then shuffle. Shuffle Green Sun's Zenith into its owner's library.";
const GLIMPSE_OF_NATURE: &str = "Whenever you cast a creature spell this turn, draw a card.";
const SOUL_WARDEN: &str = "Whenever another creature enters, you gain 1 life.";
const ZOMBIFY: &str = "Return target creature card from your graveyard to the battlefield.";
const HEROS_DOWNFALL: &str = "Destroy target creature or planeswalker.";
const MIND_SCULPT: &str = "Target opponent mills seven cards.";
const PAINTER_ORACLE: &str = "As this creature enters, choose a color.\nAll cards that aren't on the battlefield, spells, and permanents are the chosen color in addition to their other colors.";
const ARCANE_PROXY: &str = "Prototype {1}{U}{U} — 2/1 (You may cast this spell with different mana cost, color, and size. It keeps its abilities and types.)\nWhen this creature enters, if you cast it, exile target instant or sorcery card with mana value less than or equal to this creature's power from your graveyard. Copy that card. You may cast the copy without paying its mana cost.";
const TESTCARD_ORACLE: &str = "As long as Testcard is in your graveyard and you control a Forest, it's a 1/1 Insect creature in addition to its other types.";

fn grist_cost() -> ManaCost {
    ManaCost::Cost {
        generic: 1,
        shards: vec![ManaCostShard::Black, ManaCostShard::Green],
    }
}

fn cast_prohibition_count(state: &GameState, id: ObjectId) -> usize {
    let canonical = StaticDefinition::new(StaticMode::CantBeCountered);
    state.objects[&id]
        .static_definitions
        .iter_unchecked()
        .filter(|definition| **definition == canonical)
        .count()
}

fn cast_prohibition_assert_proxy(state: &GameState, id: ObjectId, prototype: bool) {
    let object = &state.objects[&id];
    // CR 718.3b + CR 718.4: the chosen form changes live characteristics,
    // while the printed baseline remains the normal form.
    assert_eq!(object.prototype_form.is_some(), prototype);
    assert_eq!(
        (object.power, object.toughness),
        if prototype {
            (Some(2), Some(1))
        } else {
            (Some(4), Some(3))
        }
    );
    assert_eq!(
        object.mana_cost,
        if prototype {
            ManaCost::Cost {
                generic: 1,
                shards: vec![ManaCostShard::Blue, ManaCostShard::Blue],
            }
        } else {
            ManaCost::Cost {
                generic: 7,
                shards: vec![],
            }
        }
    );
    assert_eq!(
        object.color,
        if prototype {
            vec![ManaColor::Blue]
        } else {
            vec![]
        }
    );
    assert_eq!(
        (object.base_power, object.base_toughness),
        (Some(4), Some(3))
    );
    assert_eq!(
        object.base_mana_cost,
        ManaCost::Cost {
            generic: 7,
            shards: vec![]
        }
    );
    assert!(object.base_color.is_empty());
    assert!(object.card_types.core_types.contains(&CoreType::Artifact));
    assert!(object.card_types.core_types.contains(&CoreType::Creature));
    assert!(object
        .card_types
        .subtypes
        .iter()
        .any(|subtype| subtype == "Wizard"));
    assert!(object.stickers.is_empty());
}

fn cast_prohibition_assert_spell(
    state: &GameState,
    id: ObjectId,
    occurrence: ObjectIncarnationRef,
) {
    let object = &state.objects[&id];
    assert_eq!(object.zone, Zone::Stack);
    assert_eq!(ObjectIncarnationRef::from_object(object), occurrence);
    assert!(object.stickers.is_empty());
    assert!(state.stack.iter().chain(state.resolving_stack_entry.iter()).any(|entry| {
        entry.id == id && entry.source_id == id
            && matches!(entry.kind, StackEntryKind::Spell { card_id, .. } if card_id == object.card_id)
    }), "current object must have an actual matching Spell entry");
}

fn cast_prohibition_restore(state: &GameState, trusted: bool) -> GameState {
    let db = shared_card_db().expect("restoration requires the integration CardDB");
    let persisted = if trusted {
        PersistedGameState::capture(state.clone())
    } else {
        PersistedGameState::Raw(Box::new(state.clone()))
    };
    let encoded = serde_json::to_string(&persisted).expect("serialize actual persisted payload");
    let decoded: PersistedGameState =
        serde_json::from_str(&encoded).expect("decode persisted payload");
    decoded
        .prepare_for_restore(PersistedRestoreFinalization::DeferUntilRehydrated)
        .expect("prepare actual persisted restore")
        .finalize_after_rehydration(|restored| {
            rehydrate_game_from_card_db_with_finalization(
                restored,
                db,
                CardDbRehydrationFinalization::Defer,
            );
            Ok(())
        })
        .expect("rehydrate and finalize actual persisted restore")
}

fn cast_prohibition_paid_proxy(prototype: bool) -> (GameRunner, ObjectId, ObjectId, ObjectId) {
    let db = shared_card_db().expect("paid Cavern test requires the integration CardDB");
    let mut scenario = main_phase();
    let cavern = scenario.add_real_card(P0, "Cavern of Souls", Zone::Hand, db);
    let proxy = scenario.add_real_card(P0, "Arcane Proxy", Zone::Hand, db);
    let counterspell = scenario.add_real_card(P1, "Counterspell", Zone::Hand, db);
    scenario.add_real_card(P1, "Unsummon", Zone::Hand, db);
    scenario.add_real_card(P1, "Unsubstantiate", Zone::Hand, db);
    let mut runner = scenario.build();
    let card_id = runner.state().objects[&cavern].card_id;
    runner
        .act(GameAction::PlayLand {
            object_id: cavern,
            card_id,
        })
        .expect("play actual Cavern");
    let WaitingFor::NamedChoice {
        player, options, ..
    } = &runner.state().waiting_for
    else {
        panic!(
            "Cavern must require its actual creature-type choice: {:?}",
            runner.state().waiting_for
        );
    };
    assert_eq!(*player, P0);
    assert!(options.iter().any(|option| option == "Wizard"));
    runner
        .act(GameAction::ChooseOption {
            choice: "Wizard".to_string(),
        })
        .expect("choose Wizard");
    assert!(runner.state().objects[&cavern]
        .chosen_attributes
        .contains(&ChosenAttribute::CreatureType("Wizard".to_string())));
    let object = &runner.state().objects[&proxy];
    assert!(object.card_types.core_types.contains(&CoreType::Creature));
    assert!(object
        .card_types
        .subtypes
        .iter()
        .any(|subtype| subtype == "Wizard"));
    assert!(object
        .printed_ref
        .as_ref()
        .is_some_and(|reference| db.get_face_by_printed_ref(reference).is_some()));
    let (_, _, grouped) = engine::ai_support::legal_actions_full(runner.state());
    let action = grouped.get(&cavern).into_iter().flatten().find(|action| matches!(action,
        GameAction::TapLandForMana { selection }
            if selection.restrictions == vec![ManaRestriction::OnlyForCreatureType("Wizard".to_string())]
                && selection.mana_type == ManaType::Blue
                && selection.output == ManaSourceOutput::Concrete(ManaType::Blue)
    )).expect("engine must issue Cavern's colored, chosen-type-restricted operation").clone();
    let GameAction::TapLandForMana { selection } = &action else {
        unreachable!()
    };
    let source = &runner.state().objects[&cavern];
    assert_eq!(selection.source, ObjectIncarnationRef::from_object(source));
    assert!(!source.tapped);
    let ability = &source.abilities[selection
        .ability_index
        .expect("actual colored ability index")];
    let Effect::Mana {
        restrictions,
        grants,
        ..
    } = &*ability.effect
    else {
        panic!("selected live colored ability must produce mana");
    };
    assert_eq!(
        restrictions,
        &vec![ManaSpendRestriction::ChosenCreatureType]
    );
    assert!(matches!(
        grants.as_slice(),
        [ManaSpellGrant::CantBeCountered { .. }]
    ));
    let meta = SpellMeta {
        types: object
            .card_types
            .core_types
            .iter()
            .map(ToString::to_string)
            .collect(),
        subtypes: object.card_types.subtypes.clone(),
        ..SpellMeta::default()
    };
    assert!(selection
        .restrictions
        .iter()
        .all(|restriction| restriction.allows_spell(&meta)));
    assert_eq!(runner.state().players[0].mana_pool.units().count(), 0);
    runner
        .act(action)
        .expect("activate engine-issued colored Cavern operation");
    assert!(
        matches!(runner.state().waiting_for, WaitingFor::Priority { player } if player == P0),
        "Concrete Blue returns directly to source-controller Priority: {:?}",
        runner.state().waiting_for
    );
    assert!(runner.state().objects[&cavern].tapped);
    let units: Vec<_> = runner.state().players[0].mana_pool.units().collect();
    assert_eq!(units.len(), 1);
    assert_eq!(units[0].source_id, cavern);
    assert_eq!(units[0].color, ManaType::Blue);
    assert_eq!(
        units[0].restrictions,
        vec![ManaRestriction::OnlyForCreatureType("Wizard".to_string())]
    );
    assert!(matches!(
        units[0].grants.as_slice(),
        [ManaSpellGrant::CantBeCountered { .. }]
    ));
    // CR 106.6 + CR 601.2h: exactly two ordinary units force the eligible
    // Cavern unit to be spent on the real prototype cost {1}{U}{U}.
    if prototype {
        add_mana(&mut runner, 0, &[ManaType::Blue, ManaType::Colorless]);
    } else {
        add_mana(&mut runner, 0, &[ManaType::Colorless; 6]);
    }
    assert!(runner.state().players[0]
        .mana_pool
        .units()
        .filter(|unit| unit.source_id != cavern)
        .all(|unit| unit.grants.is_empty() && unit.restrictions.is_empty()));
    if prototype {
        runner
            .cast(proxy)
            .casting_variant(CastingVariant::Prototype)
            .commit();
    } else {
        runner.cast(proxy).commit();
    }
    assert_eq!(
        runner.state().players[0].mana_pool.units().count(),
        0,
        "the granting unit really was spent"
    );
    let object = &runner.state().objects[&proxy];
    cast_prohibition_assert_spell(
        runner.state(),
        proxy,
        ObjectIncarnationRef::from_object(object),
    );
    assert_eq!(object.cast_from_zone, Some(Zone::Hand));
    assert!(
        object.cast_occurrence.is_some(),
        "actual finalized cast must carry its occurrence"
    );
    assert_eq!(object.prototype_form.is_some(), prototype);
    assert_eq!(
        (object.power, object.toughness),
        if prototype {
            (Some(2), Some(1))
        } else {
            (Some(4), Some(3))
        }
    );
    assert_eq!(
        object.color,
        if prototype {
            vec![ManaColor::Blue]
        } else {
            vec![]
        }
    );
    assert_eq!(
        cast_prohibition_count(runner.state(), proxy),
        1,
        "actual payment writer must stamp the live spell"
    );
    assert!(!object
        .base_static_definitions
        .iter()
        .any(|definition| *definition == StaticDefinition::new(StaticMode::CantBeCountered)));
    assert!(
        runner.state().players[0].graveyard.is_empty(),
        "Proxy ETB has no eligible own graveyard card"
    );
    (runner, cavern, proxy, counterspell)
}

fn cast_prohibition_counter(
    runner: &mut GameRunner,
    spell: ObjectId,
    counterspell: ObjectId,
) -> Vec<GameEvent> {
    for _ in 0..4 {
        if matches!(runner.state().waiting_for, WaitingFor::Priority { player } if player == P1) {
            break;
        }
        assert!(matches!(
            runner.state().waiting_for,
            WaitingFor::Priority { .. }
        ));
        runner
            .act(GameAction::PassPriority)
            .expect("yield response priority");
    }
    assert!(matches!(runner.state().waiting_for, WaitingFor::Priority { player } if player == P1));
    add_mana(runner, 1, &[ManaType::Blue, ManaType::Blue]);
    runner.cast(counterspell).target_object(spell).commit();
    assert!(
        runner.state().stack.iter().any(|entry| entry.id == spell
            && entry.source_id == spell
            && matches!(entry.kind, StackEntryKind::Spell { .. })),
        "the lower target remains an actual Spell at the counter observation boundary"
    );
    assert!(
        runner
            .state()
            .stack
            .iter()
            .any(|entry| entry.id == counterspell
                && matches!(&entry.kind, StackEntryKind::Spell { ability, .. }
            if ability.as_ref().is_some_and(|ability| ability.targets == vec![TargetRef::Object(spell)]))),
        "actual Counterspell must target the intended current Spell"
    );
    assert!(
        runner
            .state()
            .stack
            .iter()
            .any(|entry| entry.id == counterspell
                && matches!(entry.kind, StackEntryKind::Spell { .. })),
        "actual Counterspell committed with legal target"
    );
    let mut events = Vec::new();
    for _ in 0..8 {
        if runner.state().objects[&counterspell].zone != Zone::Stack {
            break;
        }
        assert!(matches!(
            runner.state().waiting_for,
            WaitingFor::Priority { .. }
        ));
        events.extend(
            runner
                .act(GameAction::PassPriority)
                .expect("resolve only Counterspell")
                .events,
        );
    }
    assert_eq!(
        runner.state().objects[&counterspell].zone,
        Zone::Graveyard,
        "Counterspell actually resolved"
    );
    events
}

fn cast_prohibition_priority(runner: &mut GameRunner, player: PlayerId) {
    for _ in 0..4 {
        if matches!(runner.state().waiting_for, WaitingFor::Priority { player: current } if current == player)
        {
            return;
        }
        assert!(matches!(
            runner.state().waiting_for,
            WaitingFor::Priority { .. }
        ));
        runner
            .act(GameAction::PassPriority)
            .expect("yield response priority");
    }
    panic!("required player did not receive priority");
}

fn cast_prohibition_response(state: &GameState, name: &str) -> ObjectId {
    state
        .objects
        .values()
        .find(|object| object.owner == P1 && object.zone == Zone::Hand && object.name == name)
        .expect("actual response card must be in P1 hand")
        .id
}

#[test]
fn cast_prohibition_restore_cavern_wizard_raw() {
    cast_prohibition_restore_cavern_wizard(false);
}

#[test]
fn cast_prohibition_restore_cavern_wizard_trusted() {
    cast_prohibition_restore_cavern_wizard(true);
}

fn cast_prohibition_restore_cavern_wizard(trusted: bool) {
    let (runner, _, proxy, counterspell) = cast_prohibition_paid_proxy(true);
    let occurrence = ObjectIncarnationRef::from_object(&runner.state().objects[&proxy]);
    let restored = cast_prohibition_restore(runner.state(), trusted);
    assert_eq!(
        restored.objects[&proxy].cast_occurrence,
        runner.state().objects[&proxy].cast_occurrence
    );
    cast_prohibition_assert_spell(&restored, proxy, occurrence);
    cast_prohibition_assert_proxy(&restored, proxy, true);
    assert_eq!(
        restored.objects[&proxy].prototype_form,
        runner.state().objects[&proxy].prototype_form
    );
    assert_eq!(
        (
            restored.objects[&proxy].power,
            restored.objects[&proxy].toughness
        ),
        (Some(2), Some(1))
    );
    let mut restored = GameRunner::from_state(restored);
    let events = cast_prohibition_counter(&mut restored, proxy, counterspell);
    // CR 112.1 + CR 701.6a: Counterspell resolves, but the paid prohibition
    // preserves this same spell; no pass is made that could resolve Proxy.
    cast_prohibition_assert_spell(restored.state(), proxy, occurrence);
    assert_eq!(cast_prohibition_count(restored.state(), proxy), 1);
    assert!(!events.iter().any(
        |event| matches!(event, GameEvent::SpellCountered { object_id, .. } if *object_id == proxy)
    ));
}

#[test]
fn cast_prohibition_restore_prototype_characteristics_raw_trusted_immediate_defer() {
    let db = shared_card_db().expect("prototype restore requires CardDB");
    for trusted in [false, true] {
        for immediate in [false, true] {
            // Plain payment isolates form restoration from the prohibition fix.
            let mut scenario = main_phase();
            let first = scenario.add_real_card(P0, "Arcane Proxy", Zone::Hand, db);
            let second = scenario.add_real_card(P0, "Arcane Proxy", Zone::Hand, db);
            let normal = scenario.add_real_card(P0, "Arcane Proxy", Zone::Hand, db);
            scenario.with_mana_pool(
                P0,
                mana(&[ManaType::Blue, ManaType::Blue, ManaType::Colorless]),
            );
            let mut runner = scenario.build();
            runner
                .cast(first)
                .casting_variant(CastingVariant::Prototype)
                .resolve()
                .assert_zone(&[first], Zone::Battlefield);
            add_mana(
                &mut runner,
                0,
                &[ManaType::Blue, ManaType::Blue, ManaType::Colorless],
            );
            runner
                .cast(second)
                .casting_variant(CastingVariant::Prototype)
                .commit();
            let occurrence = ObjectIncarnationRef::from_object(&runner.state().objects[&second]);
            let permanent_occurrence =
                ObjectIncarnationRef::from_object(&runner.state().objects[&first]);
            let marker = runner.state().objects[&first].prototype_form.clone();
            // Two actual prototype casts have independent current occurrences;
            // the normal sibling has no marker and still receives refresh.
            for id in [first, second, normal] {
                let object = runner.state_mut().objects.get_mut(&id).unwrap();
                assert!(object
                    .printed_ref
                    .as_ref()
                    .is_some_and(|reference| db.get_face_by_printed_ref(reference).is_some()));
                object.name = "stale display".to_string();
                object.base_power = Some(99);
            }
            let mut restored = if immediate {
                let persisted = if trusted {
                    PersistedGameState::capture(runner.state().clone())
                } else {
                    PersistedGameState::Raw(Box::new(runner.state().clone()))
                };
                let decoded: PersistedGameState =
                    serde_json::from_str(&serde_json::to_string(&persisted).unwrap()).unwrap();
                // This is an ordinary Immediate API call on a checked decoded
                // state, not Immediate publication inside the prepared closure.
                let mut state = decoded.into_game_state().expect("checked immediate decode");
                rehydrate_game_from_card_db_with_finalization(
                    &mut state,
                    db,
                    CardDbRehydrationFinalization::Immediate,
                );
                state
            } else {
                cast_prohibition_restore(runner.state(), trusted)
            };
            for _ in 0..2 {
                cast_prohibition_assert_spell(&restored, second, occurrence);
                assert_eq!(restored.objects[&first].zone, Zone::Battlefield);
                assert!(restored.battlefield.contains(&first));
                assert_eq!(
                    ObjectIncarnationRef::from_object(&restored.objects[&first]),
                    permanent_occurrence
                );
                cast_prohibition_assert_proxy(&restored, first, true);
                assert_eq!(restored.objects[&first].prototype_form, marker);
                assert_eq!(restored.objects[&first].name, "Arcane Proxy");
                cast_prohibition_assert_proxy(&restored, second, true);
                cast_prohibition_assert_proxy(&restored, normal, false);
                assert_eq!(restored.objects[&normal].name, "Arcane Proxy");
                rehydrate_game_from_card_db_with_finalization(
                    &mut restored,
                    db,
                    CardDbRehydrationFinalization::Immediate,
                );
                restored.layers_dirty = LayersDirty::Full;
                flush_layers(&mut restored);
            }
        }
    }
}

#[test]
fn cast_prohibition_restore_insist_real_next_spell_writer() {
    let db = shared_card_db().expect("Insist requires the regenerated integration fixture");
    let mut scenario = main_phase();
    let insist = scenario.add_real_card(P0, "Insist", Zone::Hand, db);
    let proxy = scenario.add_real_card(P0, "Arcane Proxy", Zone::Hand, db);
    let counterspell = scenario.add_real_card(P1, "Counterspell", Zone::Hand, db);
    let later_counter = scenario.add_real_card(P1, "Counterspell", Zone::Hand, db);
    let bounce = scenario.add_real_card(P1, "Unsubstantiate", Zone::Hand, db);
    scenario.add_real_card(P0, "Forest", Zone::Library, db);
    scenario.with_mana_pool(P0, mana(&[ManaType::Green]));
    let mut runner = scenario.build();
    let outcome = runner.cast(insist).resolve();
    outcome.assert_hand_drawn(P0, 1);
    outcome.assert_zone(&[insist], Zone::Graveyard);
    assert!(matches!(
        outcome.final_waiting_for(),
        WaitingFor::Priority { .. }
    ));
    assert_eq!(runner.state().pending_next_spell_modifiers.len(), 1);
    add_mana(&mut runner, 0, &[ManaType::Colorless; 7]);
    runner.cast(proxy).commit();
    assert!(runner.state().pending_next_spell_modifiers.is_empty());
    assert_eq!(cast_prohibition_count(runner.state(), proxy), 1);
    let occurrence = ObjectIncarnationRef::from_object(&runner.state().objects[&proxy]);
    let mut restored = GameRunner::from_state(cast_prohibition_restore(runner.state(), true));
    cast_prohibition_assert_spell(restored.state(), proxy, occurrence);
    let events = cast_prohibition_counter(&mut restored, proxy, counterspell);
    cast_prohibition_assert_spell(restored.state(), proxy, occurrence);
    assert_eq!(cast_prohibition_count(restored.state(), proxy), 1);
    assert!(!events.iter().any(
        |event| matches!(event, GameEvent::SpellCountered { object_id, .. } if *object_id == proxy)
    ));
    cast_prohibition_priority(&mut restored, P1);
    add_mana(&mut restored, 1, &[ManaType::Blue, ManaType::Colorless]);
    restored
        .cast(bounce)
        .target_object(proxy)
        .resolve()
        .assert_zone(&[proxy], Zone::Hand);
    assert_eq!(cast_prohibition_count(restored.state(), proxy), 0);
    cast_prohibition_priority(&mut restored, P0);
    add_mana(&mut restored, 0, &[ManaType::Colorless; 7]);
    restored.cast(proxy).commit();
    assert!(restored.state().pending_next_spell_modifiers.is_empty());
    assert_eq!(cast_prohibition_count(restored.state(), proxy), 0);
    let later_events = cast_prohibition_counter(&mut restored, proxy, later_counter);
    assert_eq!(restored.state().objects[&proxy].zone, Zone::Graveyard);
    assert!(later_events.iter().any(
        |event| matches!(event, GameEvent::SpellCountered { object_id, .. } if *object_id == proxy)
    ));
}

#[test]
fn cast_prohibition_restore_source_departure_and_live_type_loss_latch() {
    let db = shared_card_db().expect("latch tests require CardDB");
    for prototype in [false, true] {
        for depart_source in [false, true] {
            let (mut runner, cavern, proxy, counterspell) = cast_prohibition_paid_proxy(prototype);
            let occurrence = ObjectIncarnationRef::from_object(&runner.state().objects[&proxy]);
            let source_occurrence =
                ObjectIncarnationRef::from_object(&runner.state().objects[&cavern]);
            if depart_source {
                move_to_zone(runner.state_mut(), cavern, Zone::Graveyard, &mut Vec::new());
                assert_eq!(runner.state().objects[&cavern].zone, Zone::Graveyard);
                assert!(!runner.state().battlefield.contains(&cavern));
                assert!(runner.state().players[0].graveyard.contains(&cavern));
                assert_ne!(
                    ObjectIncarnationRef::from_object(&runner.state().objects[&cavern]),
                    source_occurrence
                );
            }
            let mut restored = cast_prohibition_restore(runner.state(), true);
            for mode in [
                CardDbRehydrationFinalization::Immediate,
                CardDbRehydrationFinalization::Defer,
            ] {
                for _ in 0..2 {
                    rehydrate_game_from_card_db_with_finalization(&mut restored, db, mode);
                    restored.layers_dirty = LayersDirty::Full;
                    flush_layers(&mut restored);
                    cast_prohibition_assert_spell(&restored, proxy, occurrence);
                    cast_prohibition_assert_proxy(&restored, proxy, prototype);
                    assert_eq!(cast_prohibition_count(&restored, proxy), 1);
                    assert!(!restored.objects[&proxy]
                        .base_static_definitions
                        .contains(&StaticDefinition::new(StaticMode::CantBeCountered)));
                }
            }
            if !depart_source {
                // Bounded post-payment isolation: the rider is latched to the
                // spell, so a later live type change cannot recheck its payment.
                restored
                    .objects
                    .get_mut(&proxy)
                    .unwrap()
                    .card_types
                    .core_types
                    .retain(|ty| *ty != CoreType::Creature);
                assert!(!restored.objects[&proxy]
                    .card_types
                    .core_types
                    .contains(&CoreType::Creature));
            }
            let mut restored = GameRunner::from_state(restored);
            let events = cast_prohibition_counter(&mut restored, proxy, counterspell);
            cast_prohibition_assert_spell(restored.state(), proxy, occurrence);
            assert_eq!(cast_prohibition_count(restored.state(), proxy), 1);
            assert!(!events.iter().any(|event| matches!(event, GameEvent::SpellCountered { object_id, .. } if *object_id == proxy)));
            if !depart_source {
                assert!(!restored.state().objects[&proxy]
                    .card_types
                    .core_types
                    .contains(&CoreType::Creature));
            }
        }
    }
}

#[test]
fn cast_prohibition_restore_actual_resolution_unsummon_normal_recast_countered() {
    let db = shared_card_db().expect("lifetime tests require CardDB");
    for prototype in [false, true] {
        let (mut runner, _, proxy, counterspell) = cast_prohibition_paid_proxy(prototype);
        let spell = ObjectIncarnationRef::from_object(&runner.state().objects[&proxy]);
        runner.advance_until_stack_empty();
        assert!(runner.state().stack.is_empty());
        assert!(matches!(
            runner.state().waiting_for,
            WaitingFor::Priority { .. }
        ));
        assert_eq!(runner.state().objects[&proxy].zone, Zone::Battlefield);
        assert!(runner.state().battlefield.contains(&proxy));
        // CR 112.1 + CR 400.7: resolution ends this spell's prohibition, while
        // CR 718.3b retains the chosen prototype characteristics on its permanent.
        let permanent = ObjectIncarnationRef::from_object(&runner.state().objects[&proxy]);
        assert_ne!(permanent, spell);
        assert_eq!(cast_prohibition_count(runner.state(), proxy), 0);
        cast_prohibition_assert_proxy(runner.state(), proxy, prototype);
        for trusted in [false, true] {
            let mut restored = cast_prohibition_restore(runner.state(), trusted);
            for mode in [
                CardDbRehydrationFinalization::Immediate,
                CardDbRehydrationFinalization::Defer,
            ] {
                rehydrate_game_from_card_db_with_finalization(&mut restored, db, mode);
                flush_layers(&mut restored);
                assert_eq!(
                    ObjectIncarnationRef::from_object(&restored.objects[&proxy]),
                    permanent
                );
                cast_prohibition_assert_proxy(&restored, proxy, prototype);
                assert_eq!(cast_prohibition_count(&restored, proxy), 0);
            }
        }
        let unsummon = cast_prohibition_response(runner.state(), "Unsummon");
        cast_prohibition_priority(&mut runner, P1);
        add_mana(&mut runner, 1, &[ManaType::Blue]);
        let outcome = runner.cast(unsummon).target_object(proxy).resolve();
        outcome.assert_zone(&[unsummon], Zone::Graveyard);
        outcome.assert_zone(&[proxy], Zone::Hand);
        assert!(runner.state().players[0].hand.contains(&proxy));
        assert!(!runner.state().players[1].hand.contains(&proxy));
        assert_ne!(
            ObjectIncarnationRef::from_object(&runner.state().objects[&proxy]),
            permanent
        );
        cast_prohibition_assert_proxy(runner.state(), proxy, false);
        cast_prohibition_priority(&mut runner, P0);
        add_mana(&mut runner, 0, &[ManaType::Colorless; 7]);
        runner.cast(proxy).commit();
        let later = ObjectIncarnationRef::from_object(&runner.state().objects[&proxy]);
        assert_ne!(later, spell);
        cast_prohibition_assert_spell(runner.state(), proxy, later);
        cast_prohibition_assert_proxy(runner.state(), proxy, false);
        assert_eq!(cast_prohibition_count(runner.state(), proxy), 0);
        let events = cast_prohibition_counter(&mut runner, proxy, counterspell);
        assert_eq!(runner.state().objects[&proxy].zone, Zone::Graveyard);
        assert!(events.iter().any(|event| matches!(event, GameEvent::SpellCountered { object_id, .. } if *object_id == proxy)));
    }
}

#[test]
fn cast_prohibition_restore_actual_unsubstantiate_stack_exit() {
    let (mut runner, _, proxy, _) = cast_prohibition_paid_proxy(true);
    let occurrence = ObjectIncarnationRef::from_object(&runner.state().objects[&proxy]);
    runner = GameRunner::from_state(cast_prohibition_restore(runner.state(), false));
    assert_eq!(cast_prohibition_count(runner.state(), proxy), 1);
    let bounce = cast_prohibition_response(runner.state(), "Unsubstantiate");
    cast_prohibition_priority(&mut runner, P1);
    add_mana(&mut runner, 1, &[ManaType::Blue, ManaType::Colorless]);
    let outcome = runner.cast(bounce).target_object(proxy).resolve();
    outcome.assert_zone(&[bounce], Zone::Graveyard);
    outcome.assert_zone(&[proxy], Zone::Hand);
    assert!(outcome.events().iter().all(
        |event| !matches!(event, GameEvent::SpellCountered { object_id, .. } if *object_id == proxy)
    ));
    assert!(runner.state().players[0].hand.contains(&proxy));
    assert_ne!(
        ObjectIncarnationRef::from_object(&runner.state().objects[&proxy]),
        occurrence
    );
    assert_eq!(cast_prohibition_count(runner.state(), proxy), 0);
    cast_prohibition_assert_proxy(runner.state(), proxy, false);
}

#[test]
fn cast_prohibition_restore_current_spell_authority_and_exact_definition_isolation() {
    let db = shared_card_db().expect("authority isolation requires CardDB");
    let canonical = StaticDefinition::new(StaticMode::CantBeCountered);
    for prototype in [false, true] {
        let (runner, _, proxy, _) = cast_prohibition_paid_proxy(prototype);
        // These are bounded engine-shape supplements to the public paid/counter
        // tests. Every case reaches a successful ordinary printed-face refresh.
        for mode in [
            CardDbRehydrationFinalization::Immediate,
            CardDbRehydrationFinalization::Defer,
        ] {
            for carrier in [false, true] {
                for hostile in [
                    None,
                    Some("phantom"),
                    Some("ability"),
                    Some("entry"),
                    Some("source"),
                    Some("card"),
                    Some("zone"),
                ] {
                    let mut state = runner.state().clone();
                    let occurrence = ObjectIncarnationRef::from_object(&state.objects[&proxy]);
                    if carrier {
                        // The same transfer performed by begin_resolving_stack_entry:
                        // an already-popped Spell, no TriggerFiring, one carrier.
                        assert!(state.resolving_stack_entry.is_none());
                        state.resolving_stack_entry =
                            Some(state.stack.pop_back().expect("actual paid Spell"));
                    }
                    let entry = if carrier {
                        state.resolving_stack_entry.as_mut().unwrap()
                    } else {
                        state
                            .stack
                            .iter_mut()
                            .find(|entry| entry.id == proxy)
                            .unwrap()
                    };
                    match hostile {
                        Some("ability") => {
                            entry.kind = StackEntryKind::ActivatedAbility {
                                source_id: proxy,
                                ability: Box::new(ResolvedAbility::new(
                                    Effect::NoOp,
                                    vec![],
                                    proxy,
                                    P0,
                                )),
                            }
                        }
                        Some("entry") => entry.id = ObjectId(proxy.0 + 1000),
                        Some("source") => entry.source_id = ObjectId(proxy.0 + 1000),
                        Some("card") => {
                            let StackEntryKind::Spell { card_id, .. } = &mut entry.kind else {
                                unreachable!()
                            };
                            *card_id = CardId(card_id.0 + 1000);
                        }
                        Some("zone") => state.objects.get_mut(&proxy).unwrap().zone = Zone::Hand,
                        Some("phantom") => {
                            state.stack.clear();
                            state.resolving_stack_entry = None;
                        }
                        None => {}
                        Some(other) => panic!("unhandled isolation fixture {other}"),
                    }
                    let object = state.objects.get_mut(&proxy).unwrap();
                    assert!(object
                        .printed_ref
                        .as_ref()
                        .is_some_and(|reference| db.get_face_by_printed_ref(reference).is_some()));
                    object.name = "stale display".to_string();
                    object.base_power = Some(99);
                    assert_eq!(cast_prohibition_count(&state, proxy), 1);
                    for _ in 0..2 {
                        rehydrate_game_from_card_db_with_finalization(&mut state, db, mode);
                        assert_eq!(state.objects[&proxy].name, "Arcane Proxy");
                        assert_eq!(state.objects[&proxy].base_power, Some(4));
                        assert_eq!(
                            ObjectIncarnationRef::from_object(&state.objects[&proxy]),
                            occurrence
                        );
                        assert_eq!(
                            cast_prohibition_count(&state, proxy),
                            usize::from(hostile.is_none()),
                            "{hostile:?}, carrier={carrier}, mode={mode:?}"
                        );
                        assert!(!state.objects[&proxy]
                            .base_static_definitions
                            .contains(&canonical));
                    }
                }
            }
            for payload in [
                "absent",
                "affected",
                "condition",
                "description",
                "zone",
                "base",
                "canonical_and_unrelated",
                "duplicate",
            ] {
                let mut state = runner.state().clone();
                let object = state.objects.get_mut(&proxy).unwrap();
                let mut modified = canonical.clone();
                match payload {
                    "affected" => modified.affected = Some(TargetFilter::SelfRef),
                    "condition" => modified.condition = Some(StaticCondition::DuringYourTurn),
                    "description" => modified.description = Some("different payload".to_string()),
                    "zone" => modified.active_zones = vec![Zone::Stack],
                    "base" => object.base_static_definitions = Arc::new(vec![canonical.clone()]),
                    "absent" | "canonical_and_unrelated" | "duplicate" => {}
                    other => panic!("unhandled payload fixture {other}"),
                }
                object.static_definitions = if payload == "absent" {
                    vec![]
                } else if payload == "canonical_and_unrelated" {
                    vec![
                        canonical.clone(),
                        StaticDefinition::new(StaticMode::CantBeCopied),
                    ]
                } else if payload == "duplicate" {
                    vec![canonical.clone(), canonical.clone()]
                } else {
                    vec![modified.clone()]
                }
                .into();
                object.name = "stale display".to_string();
                let expected =
                    usize::from(matches!(payload, "canonical_and_unrelated" | "duplicate"));
                for _ in 0..2 {
                    rehydrate_game_from_card_db_with_finalization(&mut state, db, mode);
                    assert_eq!(state.objects[&proxy].name, "Arcane Proxy");
                    assert_eq!(
                        cast_prohibition_count(&state, proxy),
                        expected,
                        "payload={payload}"
                    );
                    assert_eq!(state.objects[&proxy].static_definitions.len(), expected);
                    assert!(state.objects[&proxy].base_static_definitions.is_empty());
                }
            }
        }
    }
}

#[test]
fn cast_prohibition_restore_plain_colorless_and_unspent_mana_are_counterable() {
    let db = shared_card_db().expect("counter controls require CardDB");
    for selected_color in [None, Some(ManaType::Colorless), Some(ManaType::Blue)] {
        let mut scenario = main_phase();
        let cavern = scenario.add_real_card(P0, "Cavern of Souls", Zone::Hand, db);
        let proxy = scenario.add_real_card(P0, "Arcane Proxy", Zone::Hand, db);
        let counterspell = scenario.add_real_card(P1, "Counterspell", Zone::Hand, db);
        let second_source = scenario.add_real_card(P0, "Delighted Halfling", Zone::Battlefield, db);
        let mut runner = scenario.build();
        let card_id = runner.state().objects[&cavern].card_id;
        runner
            .act(GameAction::PlayLand {
                object_id: cavern,
                card_id,
            })
            .expect("play actual Cavern");
        let WaitingFor::NamedChoice {
            player, options, ..
        } = &runner.state().waiting_for
        else {
            panic!("actual Cavern choice required")
        };
        assert_eq!(*player, P0);
        assert!(options.iter().any(|option| option == "Wizard"));
        runner
            .act(GameAction::ChooseOption {
                choice: "Wizard".to_string(),
            })
            .unwrap();
        if let Some(color) = selected_color {
            let (_, _, grouped) = engine::ai_support::legal_actions_full(runner.state());
            let action = grouped[&cavern].iter().find(|action| matches!(action, GameAction::TapLandForMana { selection }
                if selection.mana_type == color && selection.output == ManaSourceOutput::Concrete(color)
                    && (color != ManaType::Colorless || selection.restrictions.is_empty())))
                .expect("real selected Cavern operation").clone();
            runner.act(action).expect("produce actual source mana");
            assert!(runner.state().objects[&cavern].tapped);
            let units: Vec<_> = runner.state().players[0].mana_pool.units().collect();
            assert_eq!(units.len(), 1);
            assert_eq!(units[0].source_id, cavern);
            assert_eq!(units[0].color, color);
            assert_eq!(!units[0].grants.is_empty(), color == ManaType::Blue);
        }
        if selected_color == Some(ManaType::Blue) {
            let (_, _, grouped) = engine::ai_support::legal_actions_full(runner.state());
            let action = grouped[&second_source].iter().find(|action| match action {
                GameAction::ActivateAbility { source_id, ability_index } if *source_id == second_source => {
                    matches!(&*runner.state().objects[&second_source].abilities[*ability_index].effect,
                        Effect::Mana { grants, .. } if matches!(grants.as_slice(), [ManaSpellGrant::CantBeCountered { .. }]))
                }
                _ => false,
            }).expect("engine issues the second source's ordinary colored mana activation").clone();
            runner
                .act(action)
                .expect("activate actual second-source ability");
            let WaitingFor::ChooseManaColor {
                player,
                choice: ManaChoicePrompt::SingleColor { options },
                ..
            } = &runner.state().waiting_for
            else {
                panic!("Halfling's ordinary activation genuinely requires a color choice");
            };
            assert_eq!(*player, P0);
            assert!(options.contains(&ManaType::Blue));
            // The primary Cavern Concrete Blue action has no second answer.
            // This separate ordinary nonland activation owns its real prompt.
            let (actions, _, _) = engine::ai_support::legal_actions_full(runner.state());
            let answer = actions
                .into_iter()
                .find(|action| {
                    matches!(
                        action,
                        GameAction::ChooseManaColor {
                            choice: ManaChoice::SingleColor(ManaType::Blue),
                            count: 1
                        }
                    )
                })
                .expect("engine offers the required secondary Blue answer");
            runner
                .act(answer)
                .expect("complete actual second-source production");
            assert!(runner.state().objects[&second_source].tapped);
            assert!(
                matches!(runner.state().waiting_for, WaitingFor::Priority { player } if player == P0)
            );
            let units: Vec<_> = runner.state().players[0].mana_pool.units().collect();
            assert_eq!(units.len(), 2);
            assert!(units.iter().any(|unit| unit.source_id == second_source
                && matches!(
                    unit.grants.as_slice(),
                    [ManaSpellGrant::CantBeCountered { .. }]
                )));
        }
        add_mana(&mut runner, 0, &[ManaType::Colorless; 7]);
        runner.cast(proxy).commit();
        assert_eq!(cast_prohibition_count(runner.state(), proxy), 0);
        if selected_color == Some(ManaType::Blue) {
            let units: Vec<_> = runner.state().players[0].mana_pool.units().collect();
            assert_eq!(
                units.len(),
                2,
                "unrestricted generic payment leaves both actual granting units unspent"
            );
            for source in [cavern, second_source] {
                assert!(units.iter().any(|unit| unit.source_id == source
                    && matches!(
                        unit.grants.as_slice(),
                        [ManaSpellGrant::CantBeCountered { .. }]
                    )));
            }
        }
        let occurrence = ObjectIncarnationRef::from_object(&runner.state().objects[&proxy]);
        let mut restored = GameRunner::from_state(cast_prohibition_restore(runner.state(), true));
        cast_prohibition_assert_spell(restored.state(), proxy, occurrence);
        assert_eq!(cast_prohibition_count(restored.state(), proxy), 0);
        let events = cast_prohibition_counter(&mut restored, proxy, counterspell);
        assert_eq!(restored.state().objects[&proxy].zone, Zone::Graveyard);
        assert!(events.iter().any(|event| matches!(event, GameEvent::SpellCountered { object_id, .. } if *object_id == proxy)));
    }
}

#[test]
fn cast_prohibition_restore_printed_intrinsic_shepherd_remains_protected() {
    let db = shared_card_db().expect("intrinsic control requires CardDB");
    let mut scenario = main_phase();
    let shepherd = scenario.add_real_card(P0, "Allosaurus Shepherd", Zone::Hand, db);
    let counterspell = scenario.add_real_card(P1, "Counterspell", Zone::Hand, db);
    scenario.with_mana_pool(P0, mana(&[ManaType::Green]));
    let mut runner = scenario.build();
    runner.cast(shepherd).commit();
    let occurrence = ObjectIncarnationRef::from_object(&runner.state().objects[&shepherd]);
    let intrinsic = runner.state().objects[&shepherd]
        .base_static_definitions
        .clone();
    assert!(intrinsic
        .iter()
        .any(|definition| definition.mode == StaticMode::CantBeCountered
            && definition.affected == Some(TargetFilter::SelfRef)));
    for trusted in [false, true] {
        let restored = cast_prohibition_restore(runner.state(), trusted);
        assert_eq!(
            restored.objects[&shepherd].base_static_definitions,
            intrinsic
        );
        let mut restored = GameRunner::from_state(restored);
        let events = cast_prohibition_counter(&mut restored, shepherd, counterspell);
        cast_prohibition_assert_spell(restored.state(), shepherd, occurrence);
        assert_eq!(
            restored.state().objects[&shepherd].base_static_definitions,
            intrinsic
        );
        assert!(!events.iter().any(|event| matches!(event, GameEvent::SpellCountered { object_id, .. } if *object_id == shepherd)));
    }
}

fn mana(types: &[ManaType]) -> Vec<ManaUnit> {
    types
        .iter()
        .map(|&t| ManaUnit::new(t, ObjectId(0), false, vec![]))
        .collect()
}

fn add_mana(runner: &mut GameRunner, player: usize, types: &[ManaType]) {
    for unit in mana(types) {
        runner.state_mut().players[player].mana_pool.add(unit);
    }
}

fn main_phase() -> GameScenario {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    scenario
}

fn grist_in_hand(scenario: &mut GameScenario, player: engine::types::player::PlayerId) -> ObjectId {
    scenario
        .add_spell_to_hand_from_oracle(player, "Grist, the Hunger Tide", false, GRIST_ORACLE)
        .as_planeswalker_with_loyalty("Grist", 3)
        .as_legendary()
        .with_mana_cost(grist_cost())
        .id()
}

fn grist_in_graveyard(scenario: &mut GameScenario) -> ObjectId {
    scenario
        .add_spell_to_graveyard(P0, "Grist, the Hunger Tide", false)
        .from_oracle_text(GRIST_ORACLE)
        .as_planeswalker_with_loyalty("Grist", 3)
        .as_legendary()
        .with_mana_cost(grist_cost())
        .id()
}

fn types_of(runner: &GameRunner, id: ObjectId) -> Vec<CoreType> {
    runner.state().objects[&id].card_types.core_types.clone()
}

fn assert_creature_card_grist(state: &engine::types::game_state::GameState, id: ObjectId) {
    let obj = &state.objects[&id];
    assert_eq!(
        obj.card_types.core_types,
        vec![CoreType::Planeswalker, CoreType::Creature],
        "zone {:?}",
        obj.zone
    );
    assert_eq!(
        obj.card_types.subtypes,
        vec!["Grist".to_string(), "Insect".to_string()]
    );
    assert_eq!((obj.power, obj.toughness), (Some(1), Some(1)));
}

fn assert_only_a_planeswalker(state: &engine::types::game_state::GameState, id: ObjectId) {
    let obj = &state.objects[&id];
    assert_eq!(obj.zone, Zone::Battlefield);
    assert_eq!(obj.card_types.core_types, vec![CoreType::Planeswalker]);
    assert_eq!(obj.card_types.subtypes, vec!["Grist".to_string()]);
    assert!(obj.card_types.supertypes.contains(&Supertype::Legendary));
    assert_eq!((obj.power, obj.toughness), (None, None));
    assert_eq!(obj.counters.get(&CounterType::Loyalty).copied(), Some(3));
}

/// I1: Grist in hand is a creature card; Duress cannot take her (CR 113.6c).
#[test]
fn hand_grist_is_a_creature_card_so_duress_cannot_take_it() {
    let mut scenario = main_phase();
    let duress = scenario
        .add_spell_to_hand_from_oracle(P0, "Duress", false, DURESS)
        .with_mana_cost(ManaCost::Cost {
            generic: 0,
            shards: vec![ManaCostShard::Black],
        })
        .id();
    let grist = grist_in_hand(&mut scenario, P1);
    let other = scenario
        .add_spell_to_hand(P1, "Other Sorcery", false)
        .with_mana_cost(ManaCost::generic(2))
        .id();
    scenario.with_mana_pool(P0, mana(&[ManaType::Black]));
    let mut runner = scenario.build();

    let outcome = runner.cast(duress).target_player(P1).resolve();
    let WaitingFor::RevealChoice { cards, .. } = outcome.final_waiting_for() else {
        panic!(
            "expected RevealChoice, got {:?}",
            outcome.final_waiting_for()
        );
    };
    // Reach guard: the noncreature nonland card is offered; Grist is a creature.
    assert_eq!(cards, &vec![other]);
    assert_eq!(outcome.zone_of(grist), Zone::Hand);
    assert!(outcome.state().objects[&grist]
        .card_types
        .core_types
        .contains(&CoreType::Creature));
}

/// I1b: a hand holding only Grist leaves Duress nothing to take.
#[test]
fn duress_against_a_hand_of_only_grist_takes_nothing() {
    let mut scenario = main_phase();
    let duress = scenario
        .add_spell_to_hand_from_oracle(P0, "Duress", false, DURESS)
        .with_mana_cost(ManaCost::Cost {
            generic: 0,
            shards: vec![ManaCostShard::Black],
        })
        .id();
    let grist = grist_in_hand(&mut scenario, P1);
    scenario.with_mana_pool(P0, mana(&[ManaType::Black]));
    let mut runner = scenario.build();

    let outcome = runner.cast(duress).target_player(P1).resolve();
    assert!(
        matches!(outcome.final_waiting_for(), WaitingFor::Priority { .. }),
        "no choice to make, got {:?}",
        outcome.final_waiting_for()
    );
    assert_eq!(outcome.zone_of(grist), Zone::Hand);
    assert_eq!(outcome.zone_count(P1, Zone::Graveyard), 0);
    // Reach guard: Duress itself resolved.
    assert_eq!(outcome.zone_of(duress), Zone::Graveyard);
}

fn glimpse_scenario() -> (GameScenario, ObjectId) {
    let mut scenario = main_phase();
    let glimpse = scenario
        .add_spell_to_hand_from_oracle(P0, "Glimpse of Nature", false, GLIMPSE_OF_NATURE)
        .with_mana_cost(ManaCost::Cost {
            generic: 0,
            shards: vec![ManaCostShard::Green],
        })
        .id();
    scenario.with_library_top(P0, &["Lib A", "Lib B", "Lib C"]);
    scenario.with_mana_pool(P0, mana(&[ManaType::Green]));
    (scenario, glimpse)
}

/// I2: Grist on the stack is a creature spell (Glimpse of Nature draws).
#[test]
fn grist_on_the_stack_is_a_creature_spell() {
    let (mut scenario, glimpse) = glimpse_scenario();
    let grist = grist_in_hand(&mut scenario, P0);
    let mut runner = scenario.build();
    runner.cast(glimpse).resolve();
    add_mana(
        &mut runner,
        0,
        &[ManaType::Black, ManaType::Green, ManaType::Colorless],
    );

    let outcome = runner.cast(grist).resolve();
    outcome.assert_hand_drawn(P0, 1);
    assert_only_a_planeswalker(outcome.state(), grist);
}

/// I2 paired controls: under the same delayed trigger a Bear draws and a
/// noncreature spell does not.
#[test]
fn glimpse_of_nature_draws_for_creature_spells_only() {
    let (mut scenario, glimpse) = glimpse_scenario();
    let bear = scenario
        .add_creature_to_hand(P0, "Bear", 2, 2)
        .with_mana_cost(ManaCost::generic(1))
        .id();
    let sorcery = scenario
        .add_spell_to_hand(P0, "Plain Sorcery", false)
        .with_mana_cost(ManaCost::generic(1))
        .id();
    let mut runner = scenario.build();
    runner.cast(glimpse).resolve();
    add_mana(&mut runner, 0, &[ManaType::Colorless, ManaType::Colorless]);
    runner.cast(sorcery).resolve().assert_hand_drawn(P0, 0);
    runner.cast(bear).resolve().assert_hand_drawn(P0, 1);
}

/// I3: Grist in the library is a green creature card of mana value 3 (Green
/// Sun's Zenith, X = 3); she arrives as a planeswalker.
#[test]
fn library_grist_is_a_green_creature_card_green_suns_zenith_finds() {
    let mut scenario = main_phase();
    let zenith = scenario
        .add_spell_to_hand_from_oracle(P0, "Green Sun's Zenith", false, GREEN_SUNS_ZENITH)
        .with_mana_cost(ManaCost::Cost {
            generic: 0,
            shards: vec![ManaCostShard::X, ManaCostShard::Green],
        })
        .id();
    let red_bear = scenario
        .add_spell_to_library_top(P0, "Red Bear", false)
        .as_creature()
        .with_mana_cost(ManaCost::generic(1))
        .with_color(vec![ManaColor::Red])
        .id();
    let grist = scenario
        .add_spell_to_library_top(P0, "Grist, the Hunger Tide", false)
        .from_oracle_text(GRIST_ORACLE)
        .as_planeswalker_with_loyalty("Grist", 3)
        .as_legendary()
        .with_mana_cost(grist_cost())
        .id();
    let warden = scenario
        .add_creature_from_oracle(P0, "Soul Warden", 1, 1, SOUL_WARDEN)
        .id();
    scenario.with_mana_pool(
        P0,
        mana(&[
            ManaType::Green,
            ManaType::Colorless,
            ManaType::Colorless,
            ManaType::Colorless,
        ]),
    );
    let mut runner = scenario.build();
    let life_before = runner.state().players[0].life;

    let outcome = runner.cast(zenith).x(3).resolve();
    let WaitingFor::SearchChoice { cards, .. } = outcome.final_waiting_for() else {
        panic!(
            "expected SearchChoice, got {:?}",
            outcome.final_waiting_for()
        );
    };
    // Reach guard: the prompt exists; the red creature card is not offered.
    assert_eq!(cards, &vec![grist]);
    assert!(!cards.contains(&red_bear));

    runner
        .act(GameAction::SelectCards { cards: vec![grist] })
        .expect("select Grist");
    runner.advance_until_stack_empty();
    assert_only_a_planeswalker(runner.state(), grist);
    // A planeswalker entering is not "another creature": Soul Warden stays quiet.
    assert!(runner.state().objects.contains_key(&warden));
    assert_eq!(runner.state().players[0].life, life_before);
}

/// I4: on the battlefield Grist is only a planeswalker, however she got there
/// (cast from hand), and a Bear entering under the same Soul Warden is the
/// paired positive.
#[test]
fn battlefield_grist_is_only_a_planeswalker() {
    let mut scenario = main_phase();
    let grist = grist_in_hand(&mut scenario, P0);
    scenario.add_creature_from_oracle(P0, "Soul Warden", 1, 1, SOUL_WARDEN);
    scenario.with_mana_pool(
        P0,
        mana(&[ManaType::Black, ManaType::Green, ManaType::Colorless]),
    );
    let mut runner = scenario.build();
    let outcome = runner.cast(grist).resolve();
    outcome.assert_life_delta(P0, 0);
    assert_only_a_planeswalker(outcome.state(), grist);

    let mut scenario = main_phase();
    let bear = scenario
        .add_creature_to_hand(P0, "Bear", 2, 2)
        .with_mana_cost(ManaCost::generic(1))
        .id();
    scenario.add_creature_from_oracle(P0, "Soul Warden", 1, 1, SOUL_WARDEN);
    scenario.with_mana_pool(P0, mana(&[ManaType::Colorless]));
    let mut runner = scenario.build();
    runner.cast(bear).resolve().assert_life_delta(P0, 1);
}

fn minus_five_scenario(loyalty: u32) -> (GameRunner, ObjectId) {
    let mut scenario = main_phase();
    let grist = scenario
        .add_planeswalker_from_oracle(P0, "Grist, the Hunger Tide", "Grist", loyalty, GRIST_ORACLE)
        .as_legendary()
        .with_mana_cost(grist_cost())
        .id();
    grist_in_graveyard(&mut scenario);
    (scenario.build(), grist)
}

/// I5: the -5 counts a Grist placed directly in the graveyard as a creature card.
#[test]
fn minus_five_counts_a_graveyard_grist_as_a_creature_card() {
    let mut scenario = main_phase();
    let grist = scenario
        .add_planeswalker_from_oracle(P0, "Grist, the Hunger Tide", "Grist", 6, GRIST_ORACLE)
        .as_legendary()
        .with_mana_cost(grist_cost())
        .id();
    grist_in_graveyard(&mut scenario);
    scenario.add_creature_to_graveyard(P0, "Bear", 2, 2);
    scenario.add_spell_to_graveyard(P0, "Sorcery", false);
    let mut runner = scenario.build();
    flush_layers_full(&mut runner);

    let outcome = runner.activate(grist, 2).resolve();
    // Grist + Bear; the sorcery is not counted.
    outcome.assert_life_delta(P1, -2);
    assert_eq!(outcome.counters(grist, CounterType::Loyalty), 1);
    assert_eq!(outcome.zone_of(grist), Zone::Battlefield);
}

/// I6: the ruling example. Paying -5 from loyalty 5 leaves Grist at 0; she dies
/// to state-based actions before the ability resolves and counts herself.
#[test]
fn minus_five_from_loyalty_five_counts_the_dying_grist() {
    let (mut runner, grist) = minus_five_scenario(5);
    flush_layers_full(&mut runner);
    let outcome = runner.activate(grist, 2).resolve();
    outcome.assert_life_delta(P1, -2);
    assert_eq!(outcome.zone_of(grist), Zone::Graveyard);
}

fn flush_layers_full(runner: &mut GameRunner) {
    runner.state_mut().layers_dirty.mark_full();
    flush_layers(runner.state_mut());
}

/// I7: Zombify legally targets the graveyard Grist (a creature card) and she
/// arrives as a planeswalker; Hero's Downfall puts her back as an Insect creature card.
#[test]
fn zombified_grist_is_a_planeswalker_again_and_a_creature_card_after_dying() {
    let mut scenario = main_phase();
    let grist = grist_in_graveyard(&mut scenario);
    let zombify = scenario
        .add_spell_to_hand_from_oracle(P0, "Zombify", false, ZOMBIFY)
        .with_mana_cost(ManaCost::Cost {
            generic: 3,
            shards: vec![ManaCostShard::Black],
        })
        .id();
    let downfall = scenario
        .add_spell_to_hand_from_oracle(P0, "Hero's Downfall", true, HEROS_DOWNFALL)
        .with_mana_cost(ManaCost::Cost {
            generic: 1,
            shards: vec![ManaCostShard::Black, ManaCostShard::Black],
        })
        .id();
    scenario.with_mana_pool(P0, mana(&[ManaType::Black; 7]));
    let mut runner = scenario.build();
    flush_layers_full(&mut runner);
    assert_creature_card_grist(runner.state(), grist);

    let outcome = runner.cast(zombify).target_objects(&[grist]).resolve();
    assert_only_a_planeswalker(outcome.state(), grist);

    let outcome = runner.cast(downfall).target_objects(&[grist]).resolve();
    assert_eq!(outcome.zone_of(grist), Zone::Graveyard);
    assert_creature_card_grist(outcome.state(), grist);
}

/// I8: a commander Grist in the command zone is a creature card, and a planeswalker
/// again once she is on the battlefield.
#[test]
fn command_zone_grist_is_a_creature_card() {
    let mut scenario = main_phase();
    let grist = grist_in_hand(&mut scenario, P0);
    scenario.with_commander(grist);
    let mut runner = scenario.build();
    assert_eq!(runner.state().objects[&grist].zone, Zone::Command);
    flush_layers_full(&mut runner);
    assert_creature_card_grist(runner.state(), grist);

    let mut events = Vec::new();
    move_to_zone(runner.state_mut(), grist, Zone::Battlefield, &mut events);
    flush_layers_full(&mut runner);
    assert_eq!(types_of(&runner, grist), vec![CoreType::Planeswalker]);
}

/// I9: a milled Grist is an Insect creature
/// card in the graveyard at once.
#[test]
fn milled_grist_is_an_insect_creature_card_at_once() {
    let mut scenario = main_phase();
    let mind_sculpt = scenario
        .add_spell_to_hand_from_oracle(P0, "Mind Sculpt", false, MIND_SCULPT)
        .with_mana_cost(ManaCost::Cost {
            generic: 1,
            shards: vec![ManaCostShard::Blue],
        })
        .id();
    let grist = scenario
        .add_spell_to_library_top(P1, "Grist, the Hunger Tide", false)
        .from_oracle_text(GRIST_ORACLE)
        .as_planeswalker_with_loyalty("Grist", 3)
        .as_legendary()
        .with_mana_cost(grist_cost())
        .id();
    scenario.add_spell_to_library_top(P1, "Filler", false);
    scenario.with_mana_pool(P0, mana(&[ManaType::Blue, ManaType::Colorless]));
    let mut runner = scenario.build();
    let library_before = runner.state().players[1].library.len();

    let outcome = runner.cast(mind_sculpt).target_player(P1).resolve();
    // Reach guard: the library was milled.
    assert!(library_before >= 2 && outcome.state().players[1].library.is_empty());
    assert_eq!(outcome.zone_of(grist), Zone::Graveyard);
    assert!(matches!(outcome.state().layers_dirty, LayersDirty::Clean));
    assert_creature_card_grist(outcome.state(), grist);
}

/// I10: the static lapses when its condition stops holding while the card stays in
/// the graveyard; types and power/toughness return to the printed values.
#[test]
fn declared_zone_static_lapses_when_the_forest_leaves() {
    let mut scenario = main_phase();
    let card = scenario
        .add_spell_to_graveyard(P0, "Testcard", false)
        .from_oracle_text(TESTCARD_ORACLE)
        .as_planeswalker_with_loyalty("Test", 3)
        .id();
    let forest = scenario.add_basic_land(P0, ManaColor::Green);
    let mut runner = scenario.build();
    flush_layers_full(&mut runner);
    let obj = &runner.state().objects[&card];
    assert_eq!(
        obj.card_types.core_types,
        vec![CoreType::Planeswalker, CoreType::Creature]
    );
    assert_eq!((obj.power, obj.toughness), (Some(1), Some(1)));

    let mut events = Vec::new();
    move_to_zone(runner.state_mut(), forest, Zone::Graveyard, &mut events);
    flush_layers_full(&mut runner);
    let obj = &runner.state().objects[&card];
    assert_eq!(obj.card_types.core_types, vec![CoreType::Planeswalker]);
    assert_eq!((obj.power, obj.toughness), (None, None));
}

/// I11: when Painter's Servant leaves, a hand card goes back to its own colour
/// (CR 611.3b + CR 613.1e: the colour effect applies only while its source is on the battlefield).
#[test]
fn painters_servant_leaving_returns_a_hand_card_to_its_own_colour() {
    let mut scenario = main_phase();
    let servant = scenario
        .add_creature_from_oracle(P0, "Painter's Servant", 1, 3, PAINTER_ORACLE)
        .as_artifact()
        .id();
    let card = scenario
        .add_spell_to_hand(P0, "Blue Card", true)
        .with_mana_cost(ManaCost::Cost {
            generic: 0,
            shards: vec![ManaCostShard::Blue],
        })
        .id();
    let mut runner = scenario.build();
    runner
        .state_mut()
        .objects
        .get_mut(&servant)
        .unwrap()
        .chosen_attributes
        .push(ChosenAttribute::Color(ManaColor::Red));
    flush_layers_full(&mut runner);
    assert_eq!(
        runner.state().objects[&card].color,
        vec![ManaColor::Blue, ManaColor::Red]
    );

    let mut events = Vec::new();
    move_to_zone(runner.state_mut(), servant, Zone::Graveyard, &mut events);
    flush_layers_full(&mut runner);
    assert_eq!(runner.state().objects[&card].color, vec![ManaColor::Blue]);
}

/// I12 (CR 718.3b): a really prototype-cast creature spell keeps its prototype
/// colours and power/toughness across layer passes under Painter's Servant.
#[test]
fn prototype_cast_spell_keeps_its_prototype_characteristics_under_painters_servant() {
    let mut scenario = main_phase();
    let servant = scenario
        .add_creature_from_oracle(P0, "Painter's Servant", 1, 3, PAINTER_ORACLE)
        .as_artifact()
        .id();
    let proxy = scenario
        .add_creature_to_hand(P0, "Arcane Proxy", 4, 3)
        .as_artifact()
        .as_creature()
        .with_mana_cost(ManaCost::generic(7))
        .from_oracle_text(ARCANE_PROXY)
        .id();
    scenario.with_mana_pool(
        P0,
        mana(&[ManaType::Blue, ManaType::Blue, ManaType::Colorless]),
    );
    let mut runner = scenario.build();
    runner
        .state_mut()
        .objects
        .get_mut(&servant)
        .unwrap()
        .chosen_attributes
        .push(ChosenAttribute::Color(ManaColor::Green));
    {
        let committed = runner
            .cast(proxy)
            .casting_variant(CastingVariant::Prototype)
            .commit();
        assert_eq!(committed.state().objects[&proxy].zone, Zone::Stack);
    }
    for pass in 0..3 {
        if pass > 0 {
            flush_layers_full(&mut runner);
        }
        let obj = &runner.state().objects[&proxy];
        // Reach guards: it really is the prototyped spell, and Green was gained.
        assert!(obj.prototype_form.is_some(), "pass {pass}");
        assert!(obj.color.contains(&ManaColor::Green), "pass {pass}");
        assert_eq!(
            obj.color,
            vec![ManaColor::Blue, ManaColor::Green],
            "pass {pass}"
        );
        assert_eq!(
            (obj.power, obj.toughness),
            (Some(2), Some(1)),
            "pass {pass}"
        );
    }
}
