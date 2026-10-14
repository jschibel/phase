//! "If C, do A and repeat this process" reached through A's gate.
//!
//! CR 608.2c: an instruction that says to repeat the process is followed again
//! from its start, here only when the clause it trails was reached (its gate
//! held). Cards (verbatim Oracle text): Grist, the Hunger Tide (+1), Cultivator
//! Colossus (#9109), Countryside Crusher, The Tale of Tamiyo.
//!
//! Library contents are forced by card type, never by seed: `K::Insect` is a
//! creature card with the Insect subtype, `K::Other` a sorcery, `K::Land` a land,
//! `K::Creature` a creature without a subtype. Rows drive `GameRunner::activate`
//! / `act` or the trigger-body route (`build_resolved_from_def` +
//! `resolve_ability_chain`) with the real parser output.

use engine::game::ability_utils::build_resolved_from_def;
use engine::game::effects::resolve_ability_chain;
use engine::game::scenario::{GameRunner, GameScenario, P0, P1};
use engine::game::zones::{create_object, move_to_zone};
use engine::parser::parse_oracle_text;
use engine::types::actions::GameAction;
use engine::types::card_type::CoreType;
use engine::types::counter::CounterType;
use engine::types::game_state::{
    AutoMayChoice, GameState, MayTriggerAutoChoiceKey, MayTriggerOrigin, WaitingFor,
};
use engine::types::identifiers::{CardId, ObjectId};
use engine::types::phase::Phase;
use engine::types::zones::Zone;

const GRIST: &str = "As long as Grist isn't on the battlefield, it's a 1/1 Insect creature in addition to its other types.\n+1: Create a 1/1 black and green Insect creature token, then mill a card. If an Insect card was milled this way, put a loyalty counter on Grist and repeat this process.\n\u{2212}2: You may sacrifice a creature. When you do, destroy target creature or planeswalker.\n\u{2212}5: Each opponent loses life equal to the number of creature cards in your graveyard.";
const COLOSSUS: &str = "Trample\nWhen this creature enters, you may put a land card from your hand onto the battlefield tapped. If you do, draw a card and repeat this process.";
const CRUSHER: &str = "At the beginning of your upkeep, reveal the top card of your library. If it's a land card, put it into your graveyard and repeat this process.";
const TAMIYO: &str = "I, II, III \u{2014} Mill two cards. If two cards that share a card type were milled this way, draw a card and repeat this process.";
const REST_IN_PEACE: &str = "When this enchantment enters, exile all graveyards.\nIf a card or token would be put into a graveyard from anywhere, exile it instead.";

#[derive(Clone, Copy, Debug)]
enum K {
    Insect,
    Other,
    Land,
    Creature,
}

fn set_kind(state: &mut GameState, id: ObjectId, k: K) {
    let o = state.objects.get_mut(&id).unwrap();
    match k {
        K::Insect => {
            o.card_types.core_types.push(CoreType::Creature);
            o.card_types.subtypes.push("Insect".to_string());
        }
        K::Other => o.card_types.core_types.push(CoreType::Sorcery),
        K::Land => o.card_types.core_types.push(CoreType::Land),
        K::Creature => o.card_types.core_types.push(CoreType::Creature),
    }
    o.base_card_types = o.card_types.clone();
}

/// Retype the top `kinds.len()` cards of P0's library, top first.
fn lib_kinds(runner: &mut GameRunner, kinds: &[K]) {
    for (i, k) in kinds.iter().enumerate() {
        let id = runner.state().players[0].library[i];
        set_kind(runner.state_mut(), id, *k);
    }
}

fn tokens_on_battlefield(st: &GameState) -> usize {
    st.objects
        .values()
        .filter(|o| o.is_token && o.zone == Zone::Battlefield)
        .count()
}

fn in_zone(st: &GameState, zone: Zone) -> usize {
    st.objects.values().filter(|o| o.zone == zone).count()
}

struct GristRun {
    loyalty: u32,
    tokens: usize,
    graveyard: usize,
    exile: usize,
}

/// Grist's +1 through `GameRunner::activate`, over a library forced to `lib` (top first).
fn grist_plus_one(lib: &[K], rest_in_peace: bool) -> GristRun {
    let mut sc = GameScenario::new();
    sc.at_phase(Phase::PreCombatMain);
    let grist = sc
        .add_planeswalker_from_oracle(P0, "Grist, the Hunger Tide", "Grist", 3, GRIST)
        .id();
    if rest_in_peace {
        sc.add_enchantment_from_oracle(P1, "Rest in Peace", REST_IN_PEACE);
    }
    for i in (0..lib.len()).rev() {
        sc.add_card_to_library_top(P0, &format!("Card{i}"));
    }
    let mut runner = sc.build();
    lib_kinds(&mut runner, lib);
    let out = runner.activate(grist, 0).resolve();
    let st = out.state();
    GristRun {
        loyalty: out.counters(grist, CounterType::Loyalty),
        tokens: tokens_on_battlefield(st),
        graveyard: st.players[0].graveyard.len(),
        exile: in_zone(st, Zone::Exile),
    }
}

// G1 + G3: the +1 repeats while the milled card is an Insect card; the Insect
// token it creates is not a milled card (so `[Other]` is one pass).
#[test]
fn grist_plus_one_repeats_while_insect_cards_are_milled() {
    use K::*;
    // Loyalty includes the +1 cost: 3 + 1 + one counter per Insect card milled.
    let r = grist_plus_one(&[Insect, Insect, Other], false);
    assert_eq!(
        (r.loyalty, r.tokens, r.graveyard),
        (6, 3, 3),
        "[Insect, Insect, Other]: three passes"
    );
    let r = grist_plus_one(&[Other], false);
    assert_eq!(
        (r.loyalty, r.tokens, r.graveyard),
        (4, 1, 1),
        "[Other]: the Insect token already created is not a milled card; one pass"
    );
    let r = grist_plus_one(&[Insect, Other], false);
    assert_eq!(
        (r.loyalty, r.tokens, r.graveyard),
        (5, 2, 2),
        "[Insect, Other]"
    );
    let r = grist_plus_one(&[Insect, Insect, Insect], false);
    assert_eq!(
        (r.loyalty, r.tokens, r.graveyard),
        (7, 4, 3),
        "[Insect x3]: the fourth pass mills nothing and stops"
    );
}

// G2: CR 701.17c - Rest in Peace exiles the milled cards; they were still milled.
#[test]
fn grist_plus_one_counts_cards_exiled_instead_of_milled() {
    use K::*;
    let r = grist_plus_one(&[Insect, Insect, Other], true);
    assert_eq!(
        (r.loyalty, r.tokens, r.graveyard, r.exile),
        (6, 3, 0, 3),
        "Rest in Peace: three passes, all three milled cards exiled"
    );
    let control = grist_plus_one(&[Insect, Insect, Other], false);
    assert_eq!(
        (control.loyalty, control.graveyard, control.exile),
        (6, 3, 0),
        "paired positive: without Rest in Peace the same cards reach the graveyard"
    );
}

// G4: the process keeps going after Grist has left the battlefield (production
// route: the stamped incarnation keeps the loyalty counter off the graveyard object).
#[test]
fn grist_plus_one_continues_after_grist_leaves_the_battlefield() {
    use K::*;
    let lib = [Insect, Insect, Other];
    let mut sc = GameScenario::new();
    sc.at_phase(Phase::PreCombatMain);
    let grist = sc
        .add_planeswalker_from_oracle(P0, "Grist, the Hunger Tide", "Grist", 3, GRIST)
        .id();
    for i in (0..lib.len()).rev() {
        sc.add_card_to_library_top(P0, &format!("Card{i}"));
    }
    let mut runner = sc.build();
    lib_kinds(&mut runner, &lib);
    runner
        .act(GameAction::ActivateAbility {
            source_id: grist,
            ability_index: 0,
            payment_mode: Default::default(),
        })
        .unwrap();
    assert_eq!(
        runner.state().stack.len(),
        1,
        "reach guard: the +1 is on the stack"
    );
    move_to_zone(runner.state_mut(), grist, Zone::Graveyard, &mut vec![]);
    runner.resolve_top();
    let st = runner.state();
    assert_eq!(
        tokens_on_battlefield(st),
        3,
        "three passes made three tokens"
    );
    assert_eq!(
        st.players[0].graveyard.len(),
        4,
        "three milled cards plus Grist herself"
    );
    assert_eq!(
        st.objects[&grist].counters.get(&CounterType::Loyalty),
        None,
        "no loyalty counter is put on the graveyard object (CR 400.7)"
    );
}

// G5: needs phase 1 - a milled Grist is an Insect card (CR 113.6c).
#[test]
fn a_milled_grist_is_an_insect_card_for_the_repeat() {
    use K::*;
    let mut sc = GameScenario::new();
    sc.at_phase(Phase::PreCombatMain);
    let grist = sc
        .add_planeswalker_from_oracle(P0, "Grist, the Hunger Tide", "Grist", 3, GRIST)
        .id();
    sc.add_card_to_library_top(P0, "Other");
    let grist2 = sc
        .add_spell_to_library_top(P0, "Grist, the Hunger Tide", false)
        .from_oracle_text(GRIST)
        .as_planeswalker_with_loyalty("Grist", 3)
        .id();
    let mut runner = sc.build();
    let other = runner.state().players[0].library[1];
    set_kind(runner.state_mut(), other, Other);
    let out = runner.activate(grist, 0).resolve();
    let st = out.state();
    assert_eq!(
        out.counters(grist, CounterType::Loyalty),
        5,
        "the milled Grist counted as an Insect card: two passes"
    );
    assert_eq!(tokens_on_battlefield(st), 2);
    assert_eq!(st.objects[&grist2].zone, Zone::Graveyard);
}

/// A trigger/ability body from verbatim Oracle text, resolved through the chain
/// driver with a library forced to `lib` (top first).
struct BodyRun {
    graveyard: usize,
    hand: usize,
    library: usize,
    idle: bool,
    resolved: bool,
}

fn body_run(text: &str, name: &str, types: &[&str], subs: &[&str], lib: &[K]) -> BodyRun {
    let mut sc = GameScenario::new();
    sc.at_phase(Phase::PreCombatMain);
    let mut runner = sc.build();
    let src = {
        let st = runner.state_mut();
        create_object(
            st,
            CardId(st.next_object_id),
            P0,
            name.into(),
            Zone::Battlefield,
        )
    };
    for (i, k) in lib.iter().enumerate() {
        let st = runner.state_mut();
        let id = create_object(
            st,
            CardId(st.next_object_id),
            P0,
            format!("Lib{i}"),
            Zone::Library,
        );
        set_kind(st, id, *k);
    }
    let t: Vec<String> = types.iter().map(|s| s.to_string()).collect();
    let s: Vec<String> = subs.iter().map(|s| s.to_string()).collect();
    let parsed = parse_oracle_text(text, name, &[], &t, &s);
    let def = match parsed.triggers.iter().find_map(|t| t.execute.clone()) {
        Some(d) => *d,
        None => parsed.abilities.first().cloned().expect("an ability body"),
    };
    let ab = build_resolved_from_def(&def, src, P0);
    let mut events = vec![];
    let res = resolve_ability_chain(runner.state_mut(), &ab, &mut events, 0);
    let st = runner.state();
    BodyRun {
        graveyard: st.players[0].graveyard.len(),
        hand: st.players[0].hand.len(),
        library: st.players[0].library.len(),
        idle: st.resolution_stack.is_empty()
            && matches!(st.waiting_for, WaitingFor::Priority { .. }),
        resolved: res.is_ok(),
    }
}

// G6: Countryside Crusher - the revealed land is put into the graveyard and the
// process repeats; a nonland stops it and stays on top.
#[test]
fn countryside_crusher_repeats_while_the_revealed_card_is_a_land() {
    use K::*;
    let crusher = |lib: &[K]| {
        body_run(
            CRUSHER,
            "Countryside Crusher",
            &["Creature"],
            &["Giant", "Warrior"],
            lib,
        )
    };
    let r = crusher(&[Land, Land, Creature]);
    assert!(r.resolved && r.idle);
    assert_eq!(
        (r.graveyard, r.library),
        (2, 1),
        "[Land, Land, Creature]: the creature stays"
    );
    let r = crusher(&[Creature]);
    assert!(r.resolved && r.idle);
    assert_eq!(
        (r.graveyard, r.library),
        (0, 1),
        "paired negative: a nonland first card repeats nothing"
    );
    let r = crusher(&[Land, Land]);
    assert!(
        r.resolved && r.idle,
        "an emptied library terminates the loop"
    );
    assert_eq!(r.graveyard, 2);
    let r = crusher(&[]);
    assert!(r.resolved && r.idle, "an empty library terminates the loop");
}

// G7: The Tale of Tamiyo - the draw and the repeat are gated on the two milled cards
// sharing a card type (the base drew unconditionally).
#[test]
fn tale_of_tamiyo_repeats_while_the_milled_cards_share_a_type() {
    use K::*;
    let tamiyo = |lib: &[K]| {
        body_run(
            TAMIYO,
            "The Tale of Tamiyo",
            &["Enchantment"],
            &["Saga"],
            lib,
        )
    };
    let r = tamiyo(&[Creature, Creature, Land, Land, Other, Creature]);
    assert!(r.resolved && r.idle);
    assert_eq!(
        (r.graveyard, r.hand, r.library),
        (4, 1, 1),
        "repeats while the milled pair shares a type; stops on Land + Other"
    );
    let r = tamiyo(&[Creature, Land, Creature]);
    assert_eq!(
        (r.graveyard, r.hand),
        (2, 0),
        "Creature + Land share no type: no draw, no repeat"
    );
    let r = tamiyo(&[Land, Land, Other, Land]);
    assert_eq!(
        (r.graveyard, r.hand),
        (3, 1),
        "a one-card final mill does not satisfy \"two cards\""
    );
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Gate {
    Prompted,
    StoredAccept,
    StoredDecline,
}

struct ColossusRun {
    trace: String,
    lands: usize,
    hand: usize,
    library: usize,
    idle: bool,
}

/// Cultivator Colossus's enters trigger body. Trace: `O` = the optional prompt,
/// `Z` = the zone choice among several hand lands. `answers` is consumed one letter
/// per optional prompt (`A` accept, `D` decline; the last letter repeats).
fn colossus_run(
    hand_lands: usize,
    lib: &[K],
    answers: &str,
    gate: Gate,
    stale_slot: bool,
) -> ColossusRun {
    let mut sc = GameScenario::new();
    sc.at_phase(Phase::PreCombatMain);
    let mut runner = sc.build();
    let src = {
        let st = runner.state_mut();
        create_object(
            st,
            CardId(st.next_object_id),
            P0,
            "Cultivator Colossus".into(),
            Zone::Battlefield,
        )
    };
    for i in 0..hand_lands {
        let st = runner.state_mut();
        let id = create_object(
            st,
            CardId(st.next_object_id),
            P0,
            format!("HandLand{i}"),
            Zone::Hand,
        );
        set_kind(st, id, K::Land);
    }
    for (i, k) in lib.iter().enumerate() {
        let st = runner.state_mut();
        let id = create_object(
            st,
            CardId(st.next_object_id),
            P0,
            format!("Lib{i}"),
            Zone::Library,
        );
        set_kind(st, id, *k);
    }
    let parsed = parse_oracle_text(
        COLOSSUS,
        "Cultivator Colossus",
        &["Trample".to_string()],
        &["Creature".to_string()],
        &["Plant".to_string(), "Wall".to_string()],
    );
    let def = parsed
        .triggers
        .iter()
        .find_map(|t| t.execute.clone())
        .expect("the enters trigger body");
    let mut ab = build_resolved_from_def(&def, src, P0);
    if gate != Gate::Prompted {
        let origin = MayTriggerOrigin::Printed { trigger_index: 0 };
        runner.state_mut().set_may_trigger_auto_choice(
            MayTriggerAutoChoiceKey {
                player: P0,
                source_id: src,
                origin: origin.clone(),
            },
            if gate == Gate::StoredAccept {
                AutoMayChoice::Accept
            } else {
                AutoMayChoice::Decline
            },
        );
        ab.set_may_trigger_origin_recursive(origin);
    }
    if stale_slot {
        runner.state_mut().repeat_process_requested = true;
    }
    let mut events = vec![];
    resolve_ability_chain(runner.state_mut(), &ab, &mut events, 0).expect("the chain resolves");
    let answers: Vec<char> = answers.chars().collect();
    let mut asked = 0usize;
    let mut trace = String::new();
    for _ in 0..80 {
        let action = match &runner.state().waiting_for {
            WaitingFor::OptionalEffectChoice { .. } => {
                let a = answers[asked.min(answers.len() - 1)];
                asked += 1;
                trace.push('O');
                GameAction::DecideOptionalEffect { accept: a == 'A' }
            }
            WaitingFor::EffectZoneChoice { cards, .. } => {
                trace.push('Z');
                GameAction::SelectCards {
                    cards: vec![cards[0]],
                }
            }
            _ => break,
        };
        runner.act(action).expect("the answer is accepted");
    }
    let st = runner.state();
    let lands = st
        .battlefield
        .iter()
        .filter(|id| {
            st.objects[id]
                .card_types
                .core_types
                .contains(&CoreType::Land)
        })
        .count();
    ColossusRun {
        trace,
        lands,
        hand: st.players[0].hand.len(),
        library: st.players[0].library.len(),
        idle: st.resolution_stack.is_empty()
            && matches!(st.waiting_for, WaitingFor::Priority { .. }),
    }
}

// G8: Cultivator Colossus (#9109) - the "you may" is re-asked every pass (phase 2),
// and the repeat is reached only when a land was put and the card drawn.
#[test]
fn cultivator_colossus_repeats_and_asks_each_pass() {
    use K::*;
    // Two lands in hand: each pass also pauses on the zone choice (drain with a choice).
    let r = colossus_run(
        2,
        &[Land, Land, Creature, Creature],
        "AD",
        Gate::Prompted,
        false,
    );
    assert_eq!(
        r.trace, "OZO",
        "accept, choose the land, then asked again and declined"
    );
    assert_eq!((r.lands, r.library, r.hand), (1, 3, 2));
    let r = colossus_run(
        2,
        &[Land, Land, Creature, Creature],
        "A",
        Gate::Prompted,
        false,
    );
    assert!(
        r.trace.starts_with("OZOZOZO"),
        "every pass asks again: {}",
        r.trace
    );
    assert_eq!((r.lands, r.hand, r.library), (4, 2, 0));
    // One land in hand: no zone choice, a prompt per pass.
    let r = colossus_run(1, &[Land, Land, Creature], "AAD", Gate::Prompted, false);
    assert_eq!(r.trace, "OOO", "decline on the third prompt ends the loop");
    assert_eq!((r.lands, r.library), (2, 1));
    // Stored decline: no prompt, no land, nothing drawn.
    let r = colossus_run(1, &[Land, Land, Creature], "A", Gate::StoredDecline, false);
    assert_eq!((r.trace.as_str(), r.lands, r.library), ("", 0, 3));
    // Stored accept: consecutive non-pausing passes run to the end without a prompt.
    let r = colossus_run(1, &[Land, Land, Creature], "A", Gate::StoredAccept, false);
    assert_eq!(r.trace, "", "a stored accept never prompts");
    assert_eq!((r.lands, r.hand, r.library), (3, 1, 0));
    assert!(r.idle);
    // Nothing to put: accepting performs nothing, the "if you do" draw does not run.
    let r = colossus_run(0, &[Land, Land], "A", Gate::Prompted, false);
    assert_eq!(
        (r.lands, r.library),
        (0, 2),
        "no repeat without a performed put"
    );
}

// G9: the request slot is cleared at the head of every pass, so a stale `true`
// (a request left behind by an earlier resolution) cannot re-ask a declined pass.
#[test]
fn a_stale_repeat_request_does_not_survive_a_declined_pass() {
    use K::*;
    let r = colossus_run(1, &[Land, Land, Creature], "D", Gate::Prompted, true);
    assert_eq!(
        r.trace, "O",
        "declined at the first prompt: no second prompt"
    );
    assert_eq!(r.lands, 0);
    let control = colossus_run(
        2,
        &[Land, Land, Creature, Creature],
        "AD",
        Gate::Prompted,
        false,
    );
    assert_eq!(
        control.trace, "OZO",
        "paired positive: the slot is set and read in a real pass"
    );
}

/// G13: a milled Grist counts at -5 through the real mill path, and the +1 keeps
/// repeating through her (phase 1 + phase 3).
fn grist_plus_one_then_minus_five(
    second_grist: bool,
    strip_second_static: bool,
) -> (u32, usize, i32, u32, Zone) {
    use K::*;
    let mut sc = GameScenario::new();
    sc.at_phase(Phase::PreCombatMain);
    let grist = sc
        .add_planeswalker_from_oracle(P0, "Grist, the Hunger Tide", "Grist", 3, GRIST)
        .id();
    sc.add_card_to_library_top(P0, "Llanowar Elves");
    let mut grist2 = None;
    if second_grist {
        grist2 = Some(
            sc.add_spell_to_library_top(P0, "Grist, the Hunger Tide", false)
                .from_oracle_text(GRIST)
                .as_planeswalker_with_loyalty("Grist", 3)
                .id(),
        );
    }
    sc.add_card_to_library_top(P0, "Haywire Mite");
    let mut runner = sc.build();
    let lib = runner.state().players[0].library.clone();
    set_kind(runner.state_mut(), lib[0], Insect);
    set_kind(runner.state_mut(), *lib.last().unwrap(), Creature);
    if strip_second_static {
        if let Some(g) = grist2 {
            runner
                .state_mut()
                .objects
                .get_mut(&g)
                .unwrap()
                .static_definitions = Default::default();
        }
    }
    let out = runner.activate(grist, 0).resolve();
    let after_plus_loyalty = out.counters(grist, CounterType::Loyalty);
    let after_plus_graveyard = out.state().players[0].graveyard.len();
    drop(out);
    // The second activation of the turn is only legal with the per-object count reset.
    runner
        .state_mut()
        .objects
        .get_mut(&grist)
        .unwrap()
        .loyalty_activations_this_turn = 0;
    let out = runner.activate(grist, 2).resolve();
    (
        after_plus_loyalty,
        after_plus_graveyard,
        out.state().players[1].life,
        out.counters(grist, CounterType::Loyalty),
        out.state().objects[&grist].zone,
    )
}

#[test]
fn a_milled_grist_repeats_the_plus_one_and_counts_at_minus_five() {
    // Library [Haywire Mite (Insect creature), Grist, Llanowar Elves (creature)].
    let (loyalty_plus, gy_plus, life, loyalty, zone) = grist_plus_one_then_minus_five(true, false);
    assert_eq!(
        (loyalty_plus, gy_plus),
        (6, 3),
        "the +1 repeats through the milled Grist (phase 3 over phase 1)"
    );
    assert_eq!(
        (life, loyalty, zone),
        (17, 1, Zone::Battlefield),
        "-5 counts Mite, the milled Grist and Elves (phase 1): 3 life lost, Grist survives on 1"
    );
    // Phase 1 control: the milled Grist's static removed, so it is not a creature card.
    let (loyalty_plus, gy_plus, life, loyalty, _) = grist_plus_one_then_minus_five(true, true);
    assert_eq!(
        (loyalty_plus, gy_plus),
        (5, 2),
        "without the static the milled Grist is no Insect card: the +1 stops there"
    );
    assert_eq!((life, loyalty), (18, 0), "and she does not count at -5");
    // Paired positive: no second Grist.
    let (loyalty_plus, _, life, loyalty, _) = grist_plus_one_then_minus_five(false, false);
    assert_eq!(loyalty_plus, 5);
    assert_eq!(
        (life, loyalty),
        (17, 0),
        "Mite, Elves and the dying Grist herself"
    );
}
