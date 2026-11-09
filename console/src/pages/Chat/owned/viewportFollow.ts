export type ViewportScrollObservation = {
  following: boolean;
  direction: number;
  distanceFromBottom: number;
  programmatic: boolean;
};

// Geometry may confirm arrival at the bottom, never cancel an upward intent.
export function followAfterScroll({
  following,
  direction,
  distanceFromBottom,
  programmatic,
}: ViewportScrollObservation): boolean {
  if (programmatic || direction === 0) return following;
  if (direction < 0) return false;
  return distanceFromBottom <= 1;
}
