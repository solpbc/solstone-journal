// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
const { readCopyTable } = require('./journal_catalog_dom.js');
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');
const {Node} = require('./entities_faces_dom.js');

module.exports = async function(manifestDir) {
  let cases = 0;
  const check = (ok, message) => { cases += 1; assert.ok(ok, message); };
  const workspace = fs.readFileSync(path.join(manifestDir,'assets/entities/workspace.html'),'utf8');
  const copy = readCopyTable(path.join(manifestDir,'../solstone-core-entities/src/data/entities_copy.json'));
  const section = new Node(null,'section');
  const previewPanel = new Node(null,'div');
  const doc = { createElement: tag => new Node(doc,tag), getElementById: id => id === 'journal-detail-voice-samples' ? section : id === 'journal-detail-merge-preview' ? previewPanel : null };
  let fail = true;
  let samples = [{key:'selected', day:'20261112', stream:'mic', stream_layout:'named', segment_key:'120000_300', source:'audio', sentence_id:7}];
  const calls = [];
  const entity = {id:'person',type:'Person'};
  const context = {document:doc, ENT_COPY:copy, currentDetailEntity:entity, ctx:{disposed:false},
    entityTrustFormat:(s,v) => Object.entries(v).reduce((t,[k,x]) => t.replaceAll('{'+k+'}',x),s),
    entityErrorMessage:(_e,fallback) => fallback,
    apiJson:async (url, init={}) => {
      calls.push({url,init});
      if (init.method === 'POST') {
        if(fail) throw new Error('refused');
        samples=[]; return {removed:1};
      }
      return {total:samples.length,items:samples,cursor:null};
    }};
  vm.createContext(context);
  vm.runInContext(workspace.slice(workspace.indexOf('let voiceSamplesRequest ='), workspace.indexOf('function openFacesPanel(')),context);
  vm.runInContext(workspace.slice(workspace.indexOf('function renderJournalEntityMergePreview('),workspace.indexOf('async function commitJournalEntityMerge(')),context);
  context.renderJournalEntityMergePreview('source','target','Target',{aliases_added:0,emails_added:0,voiceprints_added:4,voiceprints_target_total:6,notes_on_source:2,speaker_labels:3,activity_records:1});
  check(previewPanel.children[1].textContent.includes('4') && previewPanel.children[2].tagName==='BUTTON','the four samples to add are visible before the confirmation control');
  await context.loadVoiceSamples(entity);
  let rows = section.all(n => n.className === 'voice-sample-row');
  check(rows.length===1,'the owner sees the saved row');
  check(rows[0].textContent.includes('2026-11-12') && rows[0].textContent.includes('12:00:00'),'date and time are from the original segment');
  const open = rows[0].all(n => n.tagName === 'A')[0];
  check(open.href.startsWith('/app/speakers/20261112#') && !open.href.includes('speaker='),'the recording opens even if its speaker is unknown');
  const button = rows[0].all(n => n.tagName === 'BUTTON')[0];
  await button.click();
  check(!button.disabled && section.all(n => n.className==='voice-sample-row').length===1,'failure keeps the sample and makes removal retryable');
  // An appended page must not invalidate removal controls in earlier pages.
  await context.loadVoiceSamples(entity, 1, true);
  fail=false; await button.click();
  check(JSON.parse(calls.find(c=>c.init.method==='POST').init.body).key==='selected','only the selected provenance is sent');
  check(section.all(n=>n.className==='voice-sample-row').length===0,'successful removal refreshes the list and count');
  check(section.textContent.includes('(0)'), 'the remaining count is zero');
  check(previewPanel.hidden && previewPanel.children.length===0, 'removing a sample clears the stale merge count and confirmation, requiring a fresh preview');
  return cases;
};
