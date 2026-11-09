import type { ReactNode } from "react";
import {
  EmptyState as SharedEmptyState,
  type EmptyStateVariant,
} from "@/components/EmptyState";

interface EmptyStateProps {
  text: ReactNode;
  variant?: EmptyStateVariant;
  children?: ReactNode;
}

export function EmptyState({
  text,
  variant = "empty",
  children,
}: EmptyStateProps) {
  return (
    <SharedEmptyState variant={variant} description={text} action={children} />
  );
}
