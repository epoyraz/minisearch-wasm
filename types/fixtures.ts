// Strict TypeScript fixtures for the published declarations. Compiled by
// `npm run test:types` against the built pkg/; nothing here runs. Lines marked
// `@ts-expect-error` must fail to type-check, everything else must pass.
import init, {
  MiniSearchWasm,
  type JoinedResults,
  type MiniSearchJSON,
  type RawResults,
  type SearchOptions,
  type SearchResult,
  type Suggestion,
} from "../pkg/minisearch_wasm.js";

export async function readmeExamples(documents: object[]): Promise<void> {
  await init();

  const mini = new MiniSearchWasm({
    idField: "id",
    fields: ["title", "description"],
    storeFields: ["title"],
    tokenizer: "jobboard",
    searchOptions: { boost: { title: 4 }, prefix: true, fuzzy: 0.2, combineWith: "AND" },
    autoVacuum: { minDirtCount: 20 },
    logger: (level, message, code) => console.log(level, message, code),
  });
  mini.addAll(documents);
  mini.add({ id: 1, title: "hello" });
  await mini.addAllAsync(documents, { chunkSize: 100 });
  mini.addAllJSON("[]");

  // One-argument calls, as in the README.
  const hits: SearchResult[] = mini.search("software engineer");
  const first: string[] = hits[0].terms;
  const score: number = hits[0].score;
  const fieldsForTerm: string[] | undefined = hits[0].match["software"];
  const stored: unknown = hits[0].title;
  const trimmed: SearchResult[] = mini.search("x", { includeMatch: false, prefix: [false, true], fuzzy: [false, 0.2] });
  const filtered = mini.search("x", { filter: { category: "books" }, boostTerm: [2, 1] });
  const tree = mini.search({ combineWith: "AND_NOT", queries: ["a", { prefix: true, queries: ["b"] }] });
  const everything = mini.search(MiniSearchWasm.wildcard);

  const suggestions: Suggestion[] = mini.autoSuggest("softw eng");
  const fuzzySuggestions: Suggestion[] = mini.autoSuggest("softw", { fuzzy: 0.2 });
  const joined: JoinedResults = mini.searchJoined("query", false);
  const ids: unknown[] = JSON.parse(joined.ids);
  const raw: RawResults = mini.searchRaw("query");
  const rawWithOptions: RawResults = mini.searchRaw("query", { prefix: false });
  const table: unknown[] = JSON.parse(mini.docIdTable());
  const version: string = mini.idTableVersion;
  const count: number = raw.count + joined.count + mini.documentCount + mini.termCount;

  mini.replace({ id: 1, title: "changed" });
  const has: boolean = mini.has(1);
  const fields: Record<string, unknown> | undefined = mini.getStoredFields(1);
  mini.discard(1);
  mini.discardAll([2, 3]);
  mini.remove({ id: 4, title: "x" });
  mini.removeAll([{ id: 5, title: "y" }]);
  mini.removeAll();
  await mini.vacuum();
  await mini.vacuum({ batchSize: 1000, batchWait: 10 });
  const dirt: number = mini.dirtCount + mini.dirtFactor;
  const vacuuming: boolean = mini.isVacuuming;

  const json: MiniSearchJSON = mini.toJSON();
  const text: string = mini.toJSONString();
  const bytes: Uint8Array = mini.toBytes();
  const native: object = mini.toNativeJSON();
  const nativeText: string = mini.toNativeJSONString();
  const loaded: MiniSearchWasm = MiniSearchWasm.loadJSON(text, { fields: ["title", "description"] });
  const loadedAsync: MiniSearchWasm = await MiniSearchWasm.loadJSONAsync(text, { fields: ["title"] });
  const fromBytes: MiniSearchWasm = MiniSearchWasm.loadBytes(bytes);
  const fromNative: MiniSearchWasm = MiniSearchWasm.loadNativeJSON(nativeText);
  const idField: unknown = MiniSearchWasm.getDefault("idField");

  const options: SearchOptions = { combineWith: "or", maxFuzzy: 3, weights: { prefix: 0.5 } };
  void [first, score, fieldsForTerm, stored, trimmed, filtered, tree, everything, suggestions, fuzzySuggestions,
    ids, rawWithOptions, table, version, count, has, fields, dirt, vacuuming, json, loaded, loadedAsync,
    fromBytes, fromNative, idField, options];
}

export function mistakes(mini: MiniSearchWasm): void {
  // @ts-expect-error `fields` is required.
  new MiniSearchWasm({ storeFields: ["title"] });
  // @ts-expect-error misspelled option.
  mini.search("x", { prefx: true });
  // @ts-expect-error invalid combineWith.
  mini.search("x", { combineWith: "XOR" });
  // @ts-expect-error callbacks are not supported.
  mini.search("x", { filter: (result: SearchResult) => result.score > 1 });
  // @ts-expect-error score is a number.
  const wrong: string = mini.search("x")[0].score;
  // @ts-expect-error a query object needs `queries`.
  mini.search({ combineWith: "AND" });
  // @ts-expect-error loadJSON needs options.
  MiniSearchWasm.loadJSON("{}");
  // @ts-expect-error unknown tokenizer.
  new MiniSearchWasm({ fields: ["a"], tokenizer: "porter" });
  void wrong;
}
