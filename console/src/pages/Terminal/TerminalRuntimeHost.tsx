import { lazy, Suspense, useEffect, useState } from "react";
import Skeleton from "../../components/Skeleton";

const TerminalPage = lazy(() => import("./index"));

interface TerminalRuntimeHostProps {
  active: boolean;
  mobile?: boolean;
}

export default function TerminalRuntimeHost({
  active,
  mobile = false,
}: TerminalRuntimeHostProps) {
  const [started, setStarted] = useState(active);

  useEffect(() => {
    if (active) setStarted(true);
  }, [active]);

  if (!started) return null;

  return (
    <div
      data-terminal-runtime-host
      hidden={!active}
      aria-hidden={!active}
      style={{
        height: "100%",
        minHeight: 0,
        minWidth: 0,
        overflow: "hidden",
      }}
    >
      <Suspense
        fallback={
          active ? (
            <div>
              <Skeleton variant="list" count={mobile ? 5 : 6} />
            </div>
          ) : null
        }
      >
        <TerminalPage routeActive={active} />
      </Suspense>
    </div>
  );
}
