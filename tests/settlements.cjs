// Run with: node tests/settlements.cjs
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const html = fs.readFileSync('src/web/index.html', 'utf8');
const script = html.match(/<script>([\s\S]*?)<\/script>/)[1];
new vm.Script(script);
const source = script.slice(script.indexOf('function settlementGrosz('), script.indexOf('async function loadSettlements('));
const {settlementGrosz, settlementBalance} = vm.runInNewContext(source + '\n({settlementGrosz,settlementBalance})');
assert.equal(settlementGrosz('1500,01'), 150001);
assert.equal(settlementGrosz('0.29'), 29);
assert.equal(settlementGrosz('8500'), 850000);
for (const bad of ['-1', '0', '1.234', '1e3', 'Infinity', '', '1000000001']) assert.equal(settlementGrosz(bad), null);
const entries = [{amount_grosz:850000,date:'2026-09-08'},{amount_grosz:29,date:'2026-09-09'},{amount_grosz:200000,date:'2025-12-31'}];
const totals = settlementBalance({overtime_pln:49950,b2b_pln:2209.29},entries);
assert.equal(totals.paid, 1050029);
assert.equal(totals.balance, 4165900);
// A payment's date is history only; moving it to another year cannot change the wallet.
assert.equal(settlementBalance({overtime_pln:49950,b2b_pln:2209.29},entries.map(e=>({...e,date:'2027-01-01'}))).balance, totals.balance);
console.log('Settlements: OK');
