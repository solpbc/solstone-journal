# what leaves your device during an AI model request

solstone is local-first. your journal lives on a computer you choose. this doc is the plain answer to the question a privacy-motivated owner should be able to *find* rather than *ask*: when an AI model is used, what actually leaves your machine, who it goes to, and under whose terms.

short version: with the bundled local model, the model request stays on your machine. with a provider or model endpoint you choose, the specific task's prompt and relevant text or image go directly from your machine to that destination. sol pbc does not sit between your journal and the model.

## with the bundled local model: model requests stay on your machine

if you use the bundled local model, model calls go to the model running on your own machine (`localhost`) and stay there. no API key, no network call to any provider, nothing to sol pbc. the model runs where your data already is.

(this local path covers transcription too. solstone installs a local transcription model during setup, so speech becomes text on this machine, not sent anywhere. with confidential processing on, by contrast, the thinking leaves your device to be processed at that service; your journal itself never does.)

if you configure your own compatible model endpoint instead, each model request goes to the address you enter. that address may be on this machine or another computer, so the task content may leave this machine.

## with a hosted provider (Google / OpenAI / Anthropic): only that task, only to them

if you connect a hosted provider, each model request contains that task's prompt plus the context relevant to *that task*, which may include text or an image. the request goes directly from your machine to the provider's API, using **your own API key under your own provider account**.

- it is per task, not a bulk upload. that might be a transcript, screen text, a screen frame, an image you share, or a rendered document page. an image file you import goes as its original bytes, including any embedded metadata such as a photo's location. your whole journal is not sent.
- it goes **straight from your machine to the provider**. the request does not pass through a sol pbc server. sol pbc is never in the middle and never sees the request, the content, or the response.
- it uses **your key, your account**. you create the key in the provider's own developer console; solstone just stores it locally and uses it. the relationship is between you and the provider.

## what does not go to sol pbc automatically

**what the product collects: nothing extra.** no telemetry, no analytics, no usage tracking, no crash phone-home. nothing about how you use solstone is reported back to sol pbc — this is verifiable in the code.

**a support report stays local until you choose to send it.** "report this" and the support page build a draft on your machine from only the journal version, your operating system name and version, the app and route you were on, an error code when there is one, and recent error lines you can edit or delete. continuing opens `support.solstone.app` with that draft in the URL fragment, which browsers do not send to the server; the first network request that contains the report is the one you make from the website after its form is open. the journal does not register you with support, poll for tickets, or contact the support service in the background.

**sol pbc does not sit between your journal and the model on these paths.** with the bundled local model, model requests stay on your machine. with your own provider or endpoint, each request goes straight to the destination you chose.

**confidential processing is an optional service.** sol pbc operates it and other optional services. confidential processing is available to approved scouts. with it on, your journal's thinking runs off your device. your journal verifies the confidential hardware before sending. no content is retained. these services are off unless you enable them, and each is disclosed on its own terms at the point you turn it on.

**what the corporation is bound to.** Article 8 bars sol pbc from selling, licensing, sublicensing, or leasing Customer Data, including anonymized, aggregated, and de-identified forms. no targeted advertising. no behavioral profiling of you, ever. your data leaves sol pbc only in the three narrow ways the covenant allows: to a service provider, strictly as far as running the service you asked for requires; when you direct it yourself, for that particular thing; or when the law compels us. in that last case, sol pbc must make reasonable efforts to limit disclosure to the minimum required and notify you unless notice is legally barred. and if any of it is ever transferred in an acquisition, the acquirer must assume covenants no less protective than Article 8 before the deal can close.

this is not a setting you have to find and switch off: it's a covenant in sol pbc's articles of incorporation (Article 8), filed with the state. while the founder serves as a director, no amendment happens without his personal written consent. after he ceases to serve, it can be changed only to strengthen these protections, or to comply with mandatory law to the minimum strictly required.

## what happens to it then is governed by *your* agreement with the provider

this document describes the model requests. what a provider does after receiving one is governed by the agreement between you and that provider, on the account whose key you used. that is why your-key-your-account matters:

- with your own Anthropic key, the request is governed by **Anthropic's developer API terms** (the console key from `console.anthropic.com`), **not** the consumer `claude.ai` chat terms — and it's your account, so that boundary is yours to set, not ours.
- with your own OpenAI key, it's the **OpenAI platform/API terms** (`platform.openai.com`), not the `chatgpt.com` consumer terms.
- with Google, review the **Google AI / Gemini API terms** for the project and key you use; see the in-product note when you add a Gemini key.
- with the bundled local model, none of this applies to model requests, because they stay on your machine.

each provider states its own data-use and retention terms; because you bring your own key, those terms — and any controls the provider offers — are yours to read and set directly:

- Anthropic (developer API): https://www.anthropic.com/legal/commercial-terms and https://privacy.anthropic.com
- OpenAI (platform/API): https://openai.com/policies/row-terms-of-use and https://platform.openai.com/docs/guides/your-data
- Google (Gemini API): https://ai.google.dev/gemini-api/terms

solstone's job is to make the choice — and its consequences — legible. the choice itself is yours. that the provider choice is yours is the whole point of how this is built.

## the deeper story

how your data moves follows the commitments in sol pbc's articles of incorporation and the choices you make in the journal. for the structural-trust story behind how solstone is built: <https://solpbc.org>

---

*solstone is open source (AGPL-3.0). the provider request and validation code lives in [`core/crates/solstone-core-generate-wire/src/`](core/crates/solstone-core-generate-wire/src/); the provider selection code lives in [`core/crates/solstone-core-thinking/src/`](core/crates/solstone-core-thinking/src/).*
