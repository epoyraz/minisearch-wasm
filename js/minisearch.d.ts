import type { Options as OriginalOptions, SearchOptions as OriginalSearchOptions } from 'minisearch';
export type { BM25Params, CombinationOperator, LowercaseCombinationOperator, MatchInfo, SearchResult, Suggestion, VacuumOptions, VacuumConditions, AutoVacuumOptions, AsPlainObject } from 'minisearch';
import type { SearchResult, Suggestion, VacuumOptions, AsPlainObject } from 'minisearch';

export type CombineWith = import('minisearch').CombinationOperator;
export type Bm25Params = Partial<import('minisearch').BM25Params>;
export interface SearchWeights { prefix?: number; fuzzy?: number }
export type PrefixOption = OriginalSearchOptions['prefix'] | readonly boolean[];
export type FuzzyOption = OriginalSearchOptions['fuzzy'] | ReadonlyArray<boolean | number>;
export type SearchOptions = Omit<OriginalSearchOptions, 'prefix' | 'fuzzy' | 'boostTerm' | 'filter' | 'bm25' | 'weights'> & {
  prefix?: PrefixOption;
  fuzzy?: FuzzyOption;
  boostTerm?: OriginalSearchOptions['boostTerm'] | readonly number[];
  filter?: OriginalSearchOptions['filter'] | Record<string, unknown>;
  bm25?: Bm25Params;
  weights?: SearchWeights;
  includeMatch?: boolean;
};
export type Options<T = any> = Omit<OriginalOptions<T>, 'searchOptions' | 'autoSuggestOptions'> & {
  searchOptions?: SearchOptions;
  autoSuggestOptions?: SearchOptions;
  tokenizer?: 'default' | 'jobboard';
};
export type MiniSearchWasmOptions<T = any> = Options<T>;
export interface QueryCombination extends SearchOptions { queries: readonly Query[] }
export type Wildcard = typeof MiniSearchWasm.wildcard;
export type Query = string | Wildcard | QueryCombination;
export interface AddAllAsyncOptions { chunkSize?: number }
export type MiniSearchJSON = AsPlainObject;
export type LogLevel = 'debug' | 'info' | 'warn' | 'error';
export interface JoinedResults { count: number; ids: string; scores: Float64Array; terms: string }
export interface JoinedSuggestions { count: number; suggestions: string; scores: Float64Array }
export interface RawResults {
  count: number; idTableVersion: string; docIds: Uint32Array; scores: Float64Array;
  termTable: string; termOffsets: Uint32Array; termIds: Uint32Array;
}
export class MiniSearchWasm<T = any> {
  constructor(options: Options<T>);
  static readonly wildcard: unique symbol;
  static getDefault(name: string): any;
  static loadJSON<T = any>(json: string, options: Options<T>): MiniSearchWasm<T>;
  static loadJSONAsync<T = any>(json: string, options: Options<T>): Promise<MiniSearchWasm<T>>;
  static loadMiniSearchJSON<T = any>(json: string, options: Options<T>): MiniSearchWasm<T>;
  static loadNativeJSON<T = any>(json: string, options?: Options<T>): MiniSearchWasm<T>;
  static loadBytes<T = any>(bytes: Uint8Array | ArrayBuffer, options?: Options<T>): MiniSearchWasm<T>;
  readonly executionMode: 'wasm' | 'javascript';
  readonly documentCount: number;
  readonly termCount: number;
  readonly dirtCount: number;
  readonly dirtFactor: number;
  readonly isVacuuming: boolean;
  readonly idTableVersion: string;
  add(document: T): void;
  addAll(documents: readonly T[]): void;
  addAllJSON(json: string): void;
  addAllAsync(documents: readonly T[], options?: AddAllAsyncOptions): Promise<void>;
  remove(document: T): void;
  removeAll(documents?: readonly T[]): void;
  discard(id: any): void;
  discardAll(ids: readonly any[]): void;
  replace(document: T): void;
  has(id: any): boolean;
  getStoredFields(id: any): Record<string, unknown> | undefined;
  search(query: Query, options?: SearchOptions): SearchResult[];
  autoSuggest(query: string, options?: SearchOptions): Suggestion[];
  searchJoined(query: string, orMode?: boolean): JoinedResults;
  searchJoinedOpts(query: string, options?: SearchOptions): JoinedResults;
  searchRaw(query: string, options?: SearchOptions): RawResults;
  docIdTable(): string;
  autoSuggestJoined(query: string): JoinedSuggestions;
  searchCountDefault(query: string, orMode: boolean): number;
  searchCountOpts(query: string, prefix: boolean, fuzzy: boolean): number;
  vacuum(options?: VacuumOptions): Promise<void>;
  compact(): void;
  toJSON(): MiniSearchJSON;
  toJSONString(): string;
  toMiniSearchJSON(): string;
  toNativeJSON(): object;
  toNativeJSONString(): string;
  toBytes(): Uint8Array;
  free(): void;
}
// Typed structurally, so that these declarations need neither the DOM nor the
// WebAssembly library: the URL of the .wasm file (a string, URL or Request), a
// fetch Response, the file's bytes, or a compiled WebAssembly.Module.
export type WasmBytes = ArrayBuffer | ArrayBufferView;
export type InitInput = string | WasmBytes | object;
export interface InitOutput { readonly memory: { readonly buffer: ArrayBuffer }; readonly [name: string]: unknown }
export function init(input?: InitInput | Promise<InitInput> | { module_or_path: InitInput | Promise<InitInput> }): Promise<InitOutput>;
export function initSync(input: WasmBytes | object | { module: WasmBytes | object }): InitOutput;
export interface MiniSearchConstructor {
  new<T = any>(options: Options<T>): MiniSearchWasm<T>;
  (input?: Parameters<typeof init>[0]): Promise<InitOutput>;
  readonly prototype: MiniSearchWasm;
  readonly MiniSearchWasm: typeof MiniSearchWasm;
  readonly wildcard: typeof MiniSearchWasm.wildcard;
  getDefault: typeof MiniSearchWasm.getDefault;
  loadJSON: typeof MiniSearchWasm.loadJSON;
  loadJSONAsync: typeof MiniSearchWasm.loadJSONAsync;
  loadMiniSearchJSON: typeof MiniSearchWasm.loadMiniSearchJSON;
  loadNativeJSON: typeof MiniSearchWasm.loadNativeJSON;
  loadBytes: typeof MiniSearchWasm.loadBytes;
  init: typeof init;
  initSync: typeof initSync;
}
declare const MiniSearch: MiniSearchConstructor;
type MiniSearch<T = any> = MiniSearchWasm<T>;
export { MiniSearch, MiniSearch as default };
