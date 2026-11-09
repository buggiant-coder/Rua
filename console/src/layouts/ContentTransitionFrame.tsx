import { useLayoutEffect, useRef, type PropsWithChildren } from "react";

interface ContentTransitionFrameProps extends PropsWithChildren {
  transitionKey: string;
  className?: string;
}

export default function ContentTransitionFrame({
  transitionKey,
  className,
  children,
}: ContentTransitionFrameProps) {
  const frameRef = useRef<HTMLDivElement>(null);
  const previousKeyRef = useRef(transitionKey);

  useLayoutEffect(() => {
    if (previousKeyRef.current === transitionKey) return;
    previousKeyRef.current = transitionKey;

    const frame = frameRef.current;
    if (!frame) return;

    frame.classList.remove("is-entering");
    void frame.offsetWidth;
    frame.classList.add("is-entering");

    const finish = () => frame.classList.remove("is-entering");
    frame.addEventListener("animationend", finish, { once: true });
    return () => frame.removeEventListener("animationend", finish);
  }, [transitionKey]);

  return (
    <div
      ref={frameRef}
      className={`rua-content-transition-frame ${className ?? ""}`.trim()}
    >
      {children}
    </div>
  );
}
