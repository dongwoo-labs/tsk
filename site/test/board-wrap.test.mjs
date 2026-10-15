import assert from 'node:assert/strict';
import test from 'node:test';
import {markdownRows, wrapText} from '../public/board-wrap.js';
test('wrapping retains all text, aligns soft breaks, and hard-breaks oversized words',()=>{
 assert.deepEqual(wrapText('one two three',8),['one two ','three']);
 assert.deepEqual(wrapText('ABCDEFGHI',4),['ABCD','EFGH','I']);
 assert.deepEqual(wrapText('a\r\nb\rc\n',4),['a','b','c','']);
 assert.deepEqual(wrapText('界界界',4),['界界','界']);
 assert.deepEqual(wrapText('éé',1),['é','é']);
});
test('markdown rows hide markers and wrap on the rendered text like ui/markdown.rs',()=>{
 assert.deepEqual(
  markdownRows('## Plan\n1. read [the spec](https://x.dev)\n- [ ] write\n- [x] ship\n> keep **it**\n---',40),
  ['Plan','1. read the spec (https://x.dev)','☐ write','☑ ship','│ keep it','─'.repeat(40)]);
 assert.deepEqual(markdownRows('| a | b |\n|---|---|\n| c | d |',20),['a │ b','c │ d']);
 assert.deepEqual(markdownRows('10. alpha beta gamma',12),['10. alpha ','    beta ','    gamma']);
 assert.deepEqual(markdownRows('```\n**raw**\n```',20),['```','**raw**','```']);
 assert.deepEqual(markdownRows('\\*lit\\* <https://x.dev/a> *em* snake_case `a*b*c`',60),
  ['*lit* https://x.dev/a em snake_case `a*b*c`']);
 assert.deepEqual(markdownRows('plain notes',20),['plain notes']);
});
