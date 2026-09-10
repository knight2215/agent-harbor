import { describe, it, expect } from "vitest";
import type { Attachment } from "../../state/conversations";
import type { AvailableModel, ManualRoute } from "../../types";
import {
  assembleContent,
  byteLength,
  effectiveModelSupportsVision,
  MAX_CONTEXT_BYTES,
  totalBytes,
} from "./attachmentContext";

function model(providerId: string, id: string, vision: boolean): AvailableModel {
  return {
    providerId,
    model: id,
    capabilities: { streaming: true, tools: true, vision, jsonMode: false, maxContext: 8000 },
    price: { inputPerMtok: 1, outputPerMtok: 2 },
  };
}

function textAttachment(name: string, text: string): Attachment {
  return { kind: "text", name, path: `/tmp/${name}`, byteLen: byteLength(text), text };
}

describe("attachmentContext", () => {
  it("returns the draft unchanged when there are no attachments", () => {
    expect(assembleContent([], "hello")).toBe("hello");
  });

  it("prepends a delimited context block for text/repo files", () => {
    const atts: Attachment[] = [
      textAttachment("notes.txt", "line one"),
      { kind: "repo", name: "src/lib.rs", path: "/tmp/src/lib.rs", byteLen: 10, text: "pub fn x()" },
    ];
    const content = assembleContent(atts, "please review");
    expect(content).toContain("### Attached file: notes.txt");
    expect(content).toContain("line one");
    expect(content).toContain("### Repository file: src/lib.rs");
    expect(content).toContain("pub fn x()");
    // The draft is preserved after the block.
    expect(content.endsWith("please review")).toBe(true);
  });

  it("includes a vision-gated image as a labelled note (not raw bytes)", () => {
    const atts: Attachment[] = [
      { kind: "image", name: "pic.png", path: "/tmp/pic.png", byteLen: 3, base64: "Zm9v", mimeType: "image/png" },
    ];
    const content = assembleContent(atts, "describe");
    expect(content).toContain("### Attached image: pic.png");
    expect(content).toContain("image/png");
    // The base64 payload is NOT dumped into the text content.
    expect(content).not.toContain("Zm9v");
  });

  it("caps the assembled block and appends a truncation marker", () => {
    // Two large text files whose combined size exceeds the cap; only the first
    // fits, the second is dropped with a marker.
    const big = "a".repeat(MAX_CONTEXT_BYTES - 200);
    const atts: Attachment[] = [textAttachment("first.txt", big), textAttachment("second.txt", big)];
    const content = assembleContent(atts, "go");
    expect(content).toContain("### Attached file: first.txt");
    expect(content).not.toContain("### Attached file: second.txt");
    expect(content).toContain("some attached context was omitted");
    // The user's draft is always preserved in full.
    expect(content.endsWith("go")).toBe(true);
  });

  it("totalBytes sums attachment sizes", () => {
    expect(totalBytes([textAttachment("a", "12345"), textAttachment("b", "678")])).toBe(8);
  });

  it("vision gate uses the override, then the pin, then the first model", () => {
    const visionM = model("openai", "gpt-4o", true);
    const noVisionM = model("openai", "gpt-3.5", false);
    const models = [noVisionM, visionM];
    const overrideVision: ManualRoute = { providerId: "openai", model: "gpt-4o" };
    const pinNoVision: ManualRoute = { providerId: "openai", model: "gpt-3.5" };

    // Override wins.
    expect(effectiveModelSupportsVision(models, overrideVision, null)).toBe(true);
    // Pin used when no override.
    expect(effectiveModelSupportsVision(models, null, pinNoVision)).toBe(false);
    // Auto (no override, no pin) falls back to the first model (no vision here).
    expect(effectiveModelSupportsVision(models, null, null)).toBe(false);
    // No models at all -> cannot accept an image.
    expect(effectiveModelSupportsVision([], overrideVision, null)).toBe(false);
  });
});
