import { pathToFileURL } from "node:url";

export function ownerAppliedLatestAcceptance(events, owner) {
  const latest = events.flat(Infinity)
    .filter(
      (event) =>
        (event.event === "labeled" || event.event === "unlabeled") &&
        event.label?.name === "adr-accepted",
    )
    .sort((left, right) =>
      String(left.created_at).localeCompare(String(right.created_at)),
    )
    .at(-1);

  return latest?.event === "labeled" && latest.actor?.login === owner;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  let input = "";
  for await (const chunk of process.stdin) input += chunk;

  const events = JSON.parse(input);
  const accepted = ownerAppliedLatestAcceptance(events, process.argv[2]);
  process.stdout.write(`${accepted}\n`);
  process.exitCode = accepted ? 0 : 1;
}
