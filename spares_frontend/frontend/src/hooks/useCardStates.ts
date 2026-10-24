import { useEffect, useState } from 'react';
import { getCredentials, getSchedulerStates } from '../api/client';
import type { CardState, StateRole } from '../types/spares';

/** The name and role of a card state, as the scheduler logged in with lists it (with FSRS, state
    2 is `Review`). Schedulers that share a state id give it the same role, so these do for cards
    of any scheduler. Until the states are fetched, a state is named by its id and has no role. */
export function useCardStates(): { stateName: (id: number) => string; stateRole: (id: number) => StateRole | undefined } {
  const states = useSchedulerStates(schedulerName());
  const find = (id: number) => states.find(s => s.id === id);
  return {
    stateName: id => find(id)?.name ?? String(id),
    stateRole: id => find(id)?.role,
  };
}

/** The states of `name`, or none until they are fetched or if there is no scheduler. */
export function useSchedulerStates(name: string | null): CardState[] {
  const [states, setStates] = useState<CardState[]>([]);
  useEffect(() => {
    if (!name) return;
    let cancelled = false;
    // Without them, states are shown by id
    getSchedulerStates(name).then(s => { if (!cancelled) setStates(s); }, console.error);
    return () => { cancelled = true; };
  }, [name]);
  return states;
}

function schedulerName(): string | null {
  try {
    return getCredentials().schedulerName;
  } catch {
    return null;
  }
}
