// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const assert = require('assert');

function answerQuestion(page, answer = true) {
  const dialog = page.q('.convey-question');
  assert.ok(dialog, 'the question is drawn in the page before it is answered');
  const input = dialog.querySelector('input');
  if (input && answer !== null) input.value = answer;
  const button = dialog.querySelector(answer === null ? '[data-question-cancel]' : '[data-question-accept]');
  button.focus();
  button.click();
}

module.exports = { answerQuestion };
