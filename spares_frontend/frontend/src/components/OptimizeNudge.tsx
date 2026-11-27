import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { applyOptimizeProposal, discardOptimizeProposal, getOptimizeStatus } from '../api/client';
import { useAuth } from '../hooks/useAuth';
import { runOnServer } from '../local/copy';
import type { OptimizeStatusResponse } from '../types/spares';
import { errorMessage } from '../utils';
import ActionResult, { type ActionOutcome } from './ActionResult';
import { DISMISSED_KEY, readDismissed, sinceOptimized, stillDismissed, today } from '../optimizeNudge';

/**
 * Suggests optimizing the scheduler's parameters after enough reviews or days since they were
 * fitted, which can be dismissed, and offers parameters the host fitted by itself
 * (`auto_optimize`), which change nothing until applied. `onOptimize` optimizes where the page
 * can; without it, the suggestion links to the Statistics page.
 */
export default function OptimizeNudge({
  onOptimize,
  onChanged,
  revision = 0,
}: {
  onOptimize?: () => void;
  /** After applying parameters, or undoing that */
  onChanged?: () => void;
  /** Changing it asks the host again, e.g. after optimizing */
  revision?: number;
}) {
  const { credentials } = useAuth();
  const schedulerName = credentials?.schedulerName;
  const [status, setStatus] = useState<OptimizeStatusResponse | null>(null);
  const [dismissed, setDismissed] = useState(readDismissed);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [outcome, setOutcome] = useState<ActionOutcome | null>(null);

  // Errors are ignored: this is a suggestion, and the server may be offline
  const refresh = useCallback(() => {
    if (schedulerName) getOptimizeStatus(schedulerName).then(setStatus, () => {});
  }, [schedulerName]);
  useEffect(() => {
    refresh();
  }, [refresh, revision]);
  // While the host fits parameters by itself, look again for its proposal
  useEffect(() => {
    if (!status?.optimizing) return;
    const timer = setInterval(refresh, 5000);
    return () => clearInterval(timer);
  }, [status?.optimizing, refresh]);

  function dismiss() {
    if (!status) return;
    const value = { reviewCount: status.review_count, day: today() };
    try {
      localStorage.setItem(DISMISSED_KEY, JSON.stringify(value));
    } catch {
      // Without storage it stays dismissed until the page is left
    }
    setDismissed(value);
  }

  async function apply() {
    if (!schedulerName) return;
    setBusy(true);
    setError(null);
    try {
      const { result, local } = await runOnServer(() => applyOptimizeProposal(schedulerName));
      setOutcome({
        message: 'Saved the new parameters and rescheduled every card.',
        eventIds: local || result.event_id === null ? [] : [result.event_id],
      });
      onChanged?.();
      refresh();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  async function discard() {
    if (!schedulerName) return;
    setBusy(true);
    setError(null);
    try {
      await discardOptimizeProposal(schedulerName);
      refresh();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  if (outcome) return <ActionResult outcome={outcome} onReverted={() => { onChanged?.(); refresh(); }} />;
  if (!status?.optimizable) return null;
  const proposal = status.proposal;
  if (proposal)
    return (
      <div className="optimize-nudge" role="status">
        <span>
          New parameters predict your reviews better (log loss {proposal.current.log_loss.toFixed(3)} →{' '}
          {proposal.optimized.log_loss.toFixed(3)}): apply and reschedule?
        </span>
        <span className="optimize-nudge-actions">
          <button onClick={apply} disabled={busy} className="touch-target-small">
            {busy ? 'Applying…' : 'Apply and reschedule'}
          </button>
          <button onClick={discard} disabled={busy} className="touch-target-small">
            Discard
          </button>
        </span>
        {error && <span className="error-text">{error}</span>}
      </div>
    );
  if (!status.suggested || stillDismissed(status, dismissed, today())) return null;
  return (
    <div className="optimize-nudge" role="status">
      <span>{sinceOptimized(status)}: optimize now?</span>
      <span className="optimize-nudge-actions">
        {onOptimize ? (
          <button onClick={onOptimize} className="touch-target-small">
            Optimize now
          </button>
        ) : (
          <Link to="/statistics#stat-scheduler" className="touch-target-small">
            Optimize now
          </Link>
        )}
        <button onClick={dismiss} className="touch-target-small" title="Hidden until as many reviews or days have passed again">
          Dismiss
        </button>
      </span>
    </div>
  );
}
