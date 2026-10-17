import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useGameStore } from "../../../stores/gameStore.ts";
import { useMultiplayerStore } from "../../../stores/multiplayerStore.ts";
import { buildGameObject, buildObjectMap } from "../../../test/factories/gameObjectFactory.ts";
import { buildGameState, buildPlayers } from "../../../test/factories/gameStateFactory.ts";
import { CardChoiceModal } from "../CardChoiceModal.tsx";

const dispatchMock = vi.fn();

vi.mock("../../../hooks/useGameDispatch.ts", () => ({
  useGameDispatch: () => dispatchMock,
}));

describe("ChooseFromZoneChoice recipient", () => {
  beforeEach(() => {
    dispatchMock.mockClear();
    useMultiplayerStore.setState({ activePlayerId: 0 });
    const first = buildGameObject({ id: 10, card_id: 10, name: "Runeclaw Bear", zone: "Battlefield" });
    const second = buildGameObject({ id: 11, card_id: 11, name: "Silvercoat Lion", zone: "Battlefield" });
    const state = buildGameState({
      players: buildPlayers([0, 1]),
      objects: buildObjectMap(first, second),
      battlefield: [10, 11],
      waiting_for: {
        type: "ChooseFromZoneChoice",
        data: { player: 0, cards: [10, 11], count: 1, up_to: false, constraint: null, source_id: 20, reciprocal_role: null },
      },
    });
    useGameStore.setState({ gameMode: "online", gameState: state, waitingFor: state.waiting_for });
  });

  afterEach(cleanup);

  it("requires one recipient and submits the selected public card", () => {
    render(<CardChoiceModal />);
    const confirm = screen.getByRole("button", { name: /confirm/i });
    expect(confirm).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: /Silvercoat Lion/ }));
    expect(confirm).toBeEnabled();
    fireEvent.click(confirm);
    expect(dispatchMock).toHaveBeenLastCalledWith({ type: "SelectCards", data: { cards: [11] } });

    fireEvent.click(screen.getByRole("button", { name: /Silvercoat Lion/ }));
    expect(confirm).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: /Runeclaw Bear/ }));
    fireEvent.click(confirm);
    expect(dispatchMock).toHaveBeenLastCalledWith({ type: "SelectCards", data: { cards: [10] } });
  });
});
