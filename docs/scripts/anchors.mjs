// Keep mdBook's published fragments alongside VitePress's heading IDs.
export function legacyAlias(id) {
  const aliases = {
    "a-note-on-agents-md": "a-note-on-agentsmd",
    "checks-that-don-t-run-in-ci": "checks-that-dont-run-in-ci",
    "comments-in-bib": "-comments-in-bib",
  };
  if (aliases[id]) return aliases[id];
  const release = /^_(\d+)-(\d+)-(\d+)-(\d{4}-\d{2}-\d{2})$/.exec(id);
  return release
    ? `${release[1]}${release[2]}${release[3]}-${release[4]}`
    : undefined;
}
