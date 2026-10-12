import assert from "node:assert/strict";
import { test } from "node:test";
import { ownerAppliedLatestAcceptance } from "./adr-signoff-events.mjs";

const labelEvent = (event, actor, createdAt) => ({
  event,
  actor: { login: actor },
  label: { name: "adr-accepted" },
  created_at: createdAt,
});

test("accepts the latest label event when the owner applied it", () => {
  assert.equal(
    ownerAppliedLatestAcceptance(
      [labelEvent("labeled", "magnus-encoded", "2026-10-12T10:00:00Z")],
      "magnus-encoded",
    ),
    true,
  );
});

test("rejects an ADR label applied by someone else", () => {
  assert.equal(
    ownerAppliedLatestAcceptance(
      [labelEvent("labeled", "contributor", "2026-10-12T10:00:00Z")],
      "magnus-encoded",
    ),
    false,
  );
});

test("rejects unsigned ADR changes", () => {
  assert.equal(ownerAppliedLatestAcceptance([], "magnus-encoded"), false);
});

test("rejects an owner label that was later removed", () => {
  assert.equal(
    ownerAppliedLatestAcceptance(
      [
        labelEvent("labeled", "magnus-encoded", "2026-10-12T10:00:00Z"),
        labelEvent("unlabeled", "magnus-encoded", "2026-10-12T10:01:00Z"),
      ],
      "magnus-encoded",
    ),
    false,
  );
});

test("rejects a non-owner reapplying after the owner's approval was removed", () => {
  assert.equal(
    ownerAppliedLatestAcceptance(
      [
        labelEvent("labeled", "magnus-encoded", "2026-10-12T10:00:00Z"),
        labelEvent("unlabeled", "moderator", "2026-10-12T10:01:00Z"),
        labelEvent("labeled", "contributor", "2026-10-12T10:02:00Z"),
      ],
      "magnus-encoded",
    ),
    false,
  );
});

test("uses event timestamps rather than API response order", () => {
  assert.equal(
    ownerAppliedLatestAcceptance(
      [
        labelEvent("labeled", "contributor", "2026-10-12T10:02:00Z"),
        labelEvent("labeled", "magnus-encoded", "2026-10-12T10:00:00Z"),
      ],
      "magnus-encoded",
    ),
    false,
  );
});

test("handles event pages as nested arrays", () => {
  assert.equal(
    ownerAppliedLatestAcceptance(
      [
        [labelEvent("labeled", "magnus-encoded", "2026-10-12T10:00:00Z")],
        [labelEvent("unlabeled", "moderator", "2026-10-12T10:01:00Z")],
      ],
      "magnus-encoded",
    ),
    false,
  );
});
