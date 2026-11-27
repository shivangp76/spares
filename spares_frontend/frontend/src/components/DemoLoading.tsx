import { useEffect, useState, type ReactNode } from "react";
import { REPOSITORY_URL } from "../demo";
import { startLocalCopy } from "../local/copy";
import { loadDemo, resetDemo, type DemoProgress } from "../local/demo";

type Loading =
  | { kind: "loading"; progress: DemoProgress | null }
  | { kind: "ready" }
  | { kind: "failed"; message: string };

let loading: Promise<void> | null = null;

/**
 * In the demo (src/demo.ts), loads its collection into this browser before showing `children`,
 * which would otherwise ask a host that isn't there. Shows how far it has got meanwhile.
 */
export default function DemoLoading({ children }: { children: ReactNode }) {
  const [state, setState] = useState<Loading>({
    kind: "loading",
    progress: null,
  });

  useEffect(() => {
    // Once per page, as StrictMode runs effects twice
    loading ??= loadDemo((progress) =>
      setState({ kind: "loading", progress }),
    ).then(() => startLocalCopy());
    loading.then(
      () => setState({ kind: "ready" }),
      (e: unknown) => {
        loading = null;
        setState({
          kind: "failed",
          message: e instanceof Error ? e.message : String(e),
        });
      },
    );
  }, []);

  if (state.kind === "ready") return children;
  return (
    <div className="login demo-loading">
      <h2>spares demo</h2>
      {state.kind === "loading" ? (
        <>
          <p className="login-intro">
            Loading a collection of example notes into this browser. Nothing
            you do here leaves it.
          </p>
          <p role="status" className="status-line">
            {state.progress
              ? `${state.progress.step}${state.progress.total > 1 ? ` ${state.progress.done}/${state.progress.total}` : ""}…`
              : "Starting…"}
          </p>
        </>
      ) : (
        <>
          <p className="login-error" role="alert">
            {state.message}
          </p>
          <p className="login-links">
            <button onClick={() => void resetDemo()}>Start over</button>{" "}
            <a href={REPOSITORY_URL}>spares on GitHub</a>
          </p>
        </>
      )}
    </div>
  );
}
