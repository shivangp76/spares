import { useEffect, useState } from 'react';
import { getGoals } from '../api/client';
import type { GoalSearch } from '../types/spares';

/** The goals of the settings (`[goals.<name>]`) and their parts' searches, to review them with. */
export function useGoalSearches(): GoalSearch[] {
  const [goals, setGoals] = useState<GoalSearch[]>([]);
  useEffect(() => {
    // Offline, or on a server without goals, there are just none to show
    getGoals().then(setGoals).catch(() => {});
  }, []);
  return goals;
}
