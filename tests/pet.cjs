// Run with: node tests/pet.cjs
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const html = fs.readFileSync('src/web/index.html', 'utf8');
const script = html.match(/<script>([\s\S]*?)<\/script>/)[1];
new vm.Script(script);
const source = script.slice(script.indexOf('function petLanes('), script.indexOf('let lanes=[];'));
const lanes = vm.runInNewContext(source + '\npetLanes');
const rect = (left, right, top = 600, bottom = 790) =>
  ({left, right, top, bottom, width: right-left, height: bottom-top});
const check = (rects, expected, width = 1600) =>
  assert.equal(JSON.stringify(lanes(width, 800, 96, 112, rects)), JSON.stringify(expected));
check([], [[0, 1504]]);
check([rect(22, 922)], [[930, 1504]]); // Project table: walk right.
check([rect(22, 922), rect(900, 1200)], [[1208, 1504]]);
check([rect(1200, 1600)], [[0, 1096]]); // Open drawer: stay left.
check([rect(0, 1600)], []); // Hide when there is no room.
check([rect(0, 1600, 20, 400)], [[0, 1504]]); // Content above the pet.
check([], [], 80);
console.log('Pet lanes: OK');
