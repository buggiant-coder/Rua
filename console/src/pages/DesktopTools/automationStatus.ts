export function hasBundleIntegrityError(messages: string[]) {
  return messages.some((message) =>
    message.includes(
      "Desktop bundle signature or sealed component integrity is invalid",
    ),
  );
}
