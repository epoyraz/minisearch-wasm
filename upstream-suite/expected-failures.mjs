// MiniSearch tests that cannot pass against this package, and why. Everything
// else in MiniSearch's suite has to pass.
const privateState = 'reads MiniSearch\'s private fields (_index, _documentIds, _fieldLength, _dirtCount, …), which a Wasm index does not have';
const structural = 'deep-compares two instances, whose Wasm handles differ';

export const expectedFailures = new Map([
  ['MiniSearch > constructor > initializes the attributes', privateState],
  ['MiniSearch > remove > cleans up all data of the deleted document', privateState],
  ['MiniSearch > remove > cleans up the index', privateState],
  ['MiniSearch > removeAll > removes all documents from the index if called with no argument', structural],
  ['MiniSearch > discard > adjusts internal data to account for the document being discarded', privateState],
  ['MiniSearch > discard > triggers auto vacuum by default', 'sets the private _dirtCount to force a vacuum'],
  ['MiniSearch > discard > applies default settings if autoVacuum is set to true', 'sets the private _dirtCount to force a vacuum'],
  ['MiniSearch > discard > applies default settings if options are set to null', 'sets the private _dirtCount to force a vacuum'],
  ['MiniSearch > discard > vacuums until under the dirt thresholds when called multiple times', privateState],
  ['MiniSearch > discard > does not perform unnecessary vacuuming when called multiple times', privateState],
  ['MiniSearch > discard > enqueued vacuum runs without conditions if a manual vacuum was called while enqueued', privateState],
  ['MiniSearch > vacuum > cleans up discarded documents from the index', structural],
  ['MiniSearch > loadJSON > allows subclassing and changing .loadJS', 'loadJS is an undocumented upstream internal (see COMPATIBILITY.md)'],
  ['MiniSearch > loadJSONAsync > makes a MiniSearch instance that is identical to .loadJSON()', structural],
]);
