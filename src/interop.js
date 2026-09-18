// Ordinary module functions: no eval / Function constructor, including under CSP.
const defaults = {
  extractField: (document, fieldName) => document[fieldName],
  stringifyField: fieldValue => fieldValue.toString(),
  tokenize: text => text.split(/[\n\r\p{Z}\p{P}]+/u),
  processTerm: term => term.toLowerCase(),
  logger: (level, message) => { if (typeof console?.[level] === 'function') console[level](message) },
};
export function callbackDefault(name) { return defaults[name]; }
export function buildResults(args) {
  const [ids, scores, table, termIds, termOffsets, queryIds, queryOffsets, fieldIds, fieldOffsets, fieldNames, stored, includeMatch] = args;
  const parsedIds = JSON.parse(ids);
  const terms = table ? table.split('\n') : [];
  const fields = fieldNames ? fieldNames.split('\n') : [];
  const storedRows = stored ? JSON.parse(stored) : null;
  const count = scores.length;
  const out = new Array(count);
  for (let i = 0; i < count; i++) {
    const termList = [];
    for (let k = termOffsets[i]; k < termOffsets[i + 1]; k++) termList.push(terms[termIds[k]]);
    const queryList = [];
    for (let k = queryOffsets[i]; k < queryOffsets[i + 1]; k++) queryList.push(terms[queryIds[k]]);
    const result = { id: parsedIds[i], score: scores[i], terms: termList, queryTerms: queryList };
    if (includeMatch) {
      const match = {};
      for (let k = termOffsets[i]; k < termOffsets[i + 1]; k++) {
        const fieldList = [];
        for (let m = fieldOffsets[k]; m < fieldOffsets[k + 1]; m++) fieldList.push(fields[fieldIds[m]]);
        match[terms[termIds[k]]] = fieldList;
      }
      result.match = match;
    }
    if (storedRows !== null && storedRows[i] !== null) Object.assign(result, storedRows[i]);
    out[i] = result;
  }
  return out;
}
