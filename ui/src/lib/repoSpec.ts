/** True when `value` looks like a valid GitHub `owner/repo` spec.
 *
 * Mirrors `validate_repo_spec` in src/repo_clone.rs: each part is limited to
 * alphanumerics, `-`, `_` and `.`, may not start with `-` (would be parsed as
 * a flag by `gh`) and may not consist solely of dots.
 */
export function isValidRepoSpec(value: string): boolean {
  const parts = value.trim().split("/");
  if (parts.length !== 2) return false;
  return parts.every(
    (part) =>
      /^[A-Za-z0-9._-]+$/.test(part) && !part.startsWith("-") && /[^.]/.test(part),
  );
}
