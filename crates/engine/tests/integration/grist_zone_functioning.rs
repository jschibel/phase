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

use engine::game::layers::flush_layers;
use engine::game::scenario::{GameRunner, GameScenario, P0, P1};
use engine::game::zones::move_to_zone;
use engine::types::ability::ChosenAttribute;
use engine::types::actions::GameAction;
use engine::types::card_type::{CoreType, Supertype};
use engine::types::counter::CounterType;
use engine::types::game_state::{CastingVariant, LayersDirty, WaitingFor};
use engine::types::identifiers::ObjectId;
use engine::types::mana::{ManaColor, ManaCost, ManaCostShard, ManaType, ManaUnit};
use engine::types::phase::Phase;
use engine::types::zones::Zone;

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
