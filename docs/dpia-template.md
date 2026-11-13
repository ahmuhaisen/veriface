# Data protection impact assessment (DPIA) template

For projects that integrate Veriface. Copy this file into your project's documentation, fill in
every `<...>` field, delete the guidance notes (in *italics*), and have it reviewed by your data
protection officer (DPO) and legal counsel **before** processing real people's data.

This template is generic. It is aware of the EU/UK GDPR and the UAE Federal Decree-Law No. 45 of
2021 on the Protection of Personal Data (PDPL), but it is not legal advice. Check which laws apply
to you (including sector rules and any free-zone data protection regime such as DIFC or ADGM), and
the regulator's own DPIA guidance and templates, which take precedence.

What the service does and does not do is described in the [integration guide](integration-guide.md)
(section 10), [licensing and privacy](licensing-and-privacy.md) and the
[threat model](threat-model.md). Copy facts from there; do not restate them from memory.

---

## 0. Document control

| Field | Value |
|---|---|
| Project / product | `<name>` |
| Controller (legal entity) | `<name, address>` |
| Joint controllers / processors | `<e.g. hosting provider, the party operating the face service, support vendors>` |
| DPO / privacy contact | `<name, contact>` |
| Author | `<name, role>` |
| Version and date | `<x.y, yyyy-mm-dd>` |
| Review date | `<at least yearly, and on any change listed in section 11>` |
| Veriface versions assessed | Binary `<engine.version>`, catalogue `<catalogueVersion>`, profile `<name version sha256>`, calibration `<id, status>`, model review status `<pending/approved>` |

## 1. Is a DPIA required?

*Processing biometric data to uniquely identify or verify a person is special-category data under
GDPR Art. 9 and sensitive personal data under the UAE PDPL. Systematic processing of such data, new
technology, and decisions affecting access to services are all recognised DPIA triggers (GDPR
Art. 35; PDPL Art. 21 for processing likely to pose a high risk, including large-scale sensitive
data). Treat a DPIA as required unless your DPO documents otherwise.*

| Trigger | Applies? | Note |
|---|---|---|
| Biometric data used to verify identity | Yes | Selfie vs ID-document portrait, 1:1 |
| Sensitive / special-category data | Yes | Face images, comparison results; ID documents may reveal nationality, age, and other attributes |
| New technology | `<yes/no>` | |
| Large scale | `<yes/no: expected volume per day / year>` | |
| Automated decision with significant effect | `<yes/no: see section 7>` | |
| Vulnerable data subjects (minors, employees, patients ...) | `<yes/no>` | |
| Cross-border transfer | `<yes/no>` | |

## 2. Description of the processing

### 2.1 Purpose

`<Why you verify identity, the business process it supports, and what happens if the person is
not verified. Be specific: "confirm that the visitor presenting the ID is its holder before
issuing a building pass", not "security".>`

*Purpose limitation: the results must not be reused for other purposes (for example building a
face gallery, profiling, or identification of unknown people). The service supports 1:1
verification only and returns no embeddings.*

### 2.2 Nature and scope

| Item | Value |
|---|---|
| Data subjects | `<customers, visitors, employees, contractors ...>` |
| Expected volume | `<verifications per day, peak per hour>` |
| Geography | `<countries of data subjects and of processing>` |
| Capture setting | `<remote self-service / supervised by staff / kiosk>` |
| Endpoints used | `<POST /v1/quality, /v1/document-portraits, /v1/verifications>` |
| Profile and operating point | `<e.g. standard, fmr-1e-4>` |

### 2.3 Data categories

| Category | Examples | Source | Sensitive? | Stored by you? | Retention |
|---|---|---|---|---|---|
| Selfie image | JPEG from the capture SDK | Data subject (camera) | **Yes, biometric** | `<yes/no>` | `<period or "not stored">` |
| ID document image | Card or passport data page photo | Data subject | **Yes**: biometric portrait plus identity data (name, number, nationality, date of birth) | `<yes/no>` | `<>` |
| Normalised output images (`output=true`) | Portrait crops returned by the service | Service | **Yes, biometric** | `<yes/no>` | `<>` |
| Face geometry in responses | `face.box`, `landmarks5`, `pose` | Service | **Treat as biometric** | `<yes/no>` | `<>` |
| Similarity score | `score` (cosine similarity) | Service | **Treat as biometric-derived** | `<yes/no>` | `<>` |
| Outcome and reasons | `outcome`, `decision`, `reasons`, check statuses | Service | Personal data | `<yes, audit>` | `<>` |
| Versions and ids | `requestId`, `engine.version`, `profile.sha256`, `calibration.id` | Service | Personal data when linked to a person | `<yes, audit>` | `<>` |
| Capture diagnostics | SDK `guidance`, `signals` (camera label, timing) | SDK | Personal data; device information | `<yes/no>` | `<>` |
| Face embeddings / templates | — | **Never produced for you; never store your own** | — | **No** | — |
| Account / case data | `<user id, case id>` | Your system | `<>` | Yes | `<>` |

### 2.4 Data flow

```
data subject's device ──(1)──► your frontend (SDK, in-browser guidance only)
                       ──(2)──► your backend ──(3)──► Veriface service (in memory only)
                                    │ ◄────────(4)── result JSON
                                    ├──(5)──► your audit store
                                    └──(6)──► reviewer (only for review / rejected / retake escalations)
```

| Step | What moves | Where it is processed | Transport security | Stored? |
|---|---|---|---|---|
| (1) | Camera frames | In the browser only (MediaPipe in a Web Worker); SDK makes no network requests other than its own assets | n/a (local) | No |
| (2) | Selfie and document JPEGs | Your backend | `<HTTPS, TLS version>` | `<>` |
| (3) | Same images + API key | Veriface host `<location, operator>` | `<loopback / TLS proxy / private network>` | **No**: processed in memory, not stored, cached or logged |
| (4) | Result JSON (no embeddings) | Your backend | as (3) | `<>` |
| (5) | `<fields>` | `<system>` | `<>` | `<retention>` |
| (6) | `<images? results?>` | `<review tool, who>` | `<>` | `<>` |

*Attach a diagram of your real architecture, including hosting locations, backups and any
third-party processors.*

### 2.5 What Veriface guarantees (copy, then verify against the versions you deploy)

- Stateless: no images, embeddings or templates are stored, cached or logged.
- Logs and metrics contain request ids, timings, project names, check ids, statuses and outcomes
  only; never images, embeddings, scores, check values or face geometry.
- Embeddings are never returned.
- No network access at runtime; models are local files verified by SHA-256 at startup.
- Runs on infrastructure you (or your operator) control.

## 3. Legal basis

| Question | Answer |
|---|---|
| Lawful basis for processing personal data (GDPR Art. 6; PDPL Art. 4 and its exceptions) | `<e.g. contract, legal obligation, legitimate interests (with balancing test)>` |
| Condition for special-category / sensitive data (GDPR Art. 9(2); PDPL rules on sensitive data) | `<e.g. explicit consent; a specific legal obligation or authorisation; substantial public interest under national law>` |
| If consent: how is it explicit, specific, informed, freely given and withdrawable? | `<consent text, where it is shown, how it is recorded, how withdrawal works>` |
| Is consent freely given? (power imbalance, e.g. employees) | `<analysis; offer of an alternative>` |
| Non-biometric alternative offered | `<e.g. manual ID check by staff>` |
| Sector or local rules | `<banking, health, government, free-zone regimes>` |

## 4. Necessity and proportionality

| Question | Answer |
|---|---|
| Why is face verification necessary for the purpose? | `<>` |
| Less intrusive alternatives considered and why rejected | `<>` |
| Data minimisation: do you need to keep the images at all? | `<prefer not storing; if stored, why and for how long>` |
| Do you send only what is needed? | `<e.g. no SDK burst frames; output=false unless needed>` |
| Accuracy: what happens to people wrongly rejected? | `<human review path; see section 7>` |
| Transparency: privacy notice text and where it is shown | `<>` |
| Retention schedule and deletion mechanism | `<>` |
| Processors and contracts (Art. 28 terms) | `<>` |
| International transfers and safeguards | `<adequacy, SCCs, PDPL transfer rules, or "none">` |

## 5. Data subject rights

| Right | How it is met | Note specific to this processing |
|---|---|---|
| Information (GDPR Art. 13/14; PDPL) | `<privacy notice>` | Say that a face comparison is performed, by whom, and what happens on failure |
| Access | `<process>` | You can provide stored images, outcomes and reasons; the service holds nothing |
| Rectification | `<process>` | A wrong outcome is corrected by re-verification or human review |
| Erasure | `<process>` | Delete your stored images, responses and audit entries per policy; nothing to delete in the service |
| Restriction / objection | `<process>` | Offer the non-biometric alternative |
| Withdraw consent | `<process>` | Stop processing; delete images unless another basis applies |
| Not to be subject to solely automated decisions (GDPR Art. 22; PDPL equivalent) | `<human review path>` | See section 7 |
| Complaint to the regulator | `<regulator, contact>` | |

## 6. Risks to data subjects

*Assess likelihood (remote / possible / probable) and severity (minimal / significant / severe)
before and after mitigations. Add risks specific to your context.*

| # | Risk | Likelihood | Severity | Mitigations (service) | Mitigations (your project) | Residual |
|---|---|---|---|---|---|---|
| R1 | Breach of stored face images or ID documents | `<>` | Severe | Service stores none | `<don't store / encrypt at rest / access control / retention>` | `<>` |
| R2 | Interception in transit | `<>` | Severe | Plain HTTP service: TLS must be added in front when off-host | `<HTTPS end to end; loopback or TLS to the service>` | `<>` |
| R3 | Genuine person wrongly rejected (false non-match), leading to denial of service or embarrassment | `<>` | Significant | Outcome `rejected` is a recommendation; reasons provided; `retake` for poor captures. **Thresholds are provisional** | `<retries, human review, alternative path, neutral wording>` | `<>` |
| R4 | Impostor wrongly verified (false match), including replay with a photo of the ID | `<>` | Significant / severe | Operating point (`targetFmr` is a target while provisional); replay risk signals turn suspicious matches into `review` | `<supervised capture where risk is high; human review; attempt limits>` | `<>` |
| R5 | Higher error rates for some demographic groups (skin tone, age, head coverings, glasses) | `<>` | Significant | Lab evaluation plan with demographic reporting (ISO/IEC 19795-10); the `standard` profile flags that strict under-exposure limits penalise darker skin tones; religious head coverings not penalised (`HEAD_COVERING` off by default) | `<monitor retake/reject rates by channel; human review; report issues to the Veriface team>` | `<>` |
| R6 | Presentation attacks (masks, deepfakes, injected video) | `<>` | Severe | **Not covered in v1** beyond replay risk signals; no certified PAD | `<restrict to supervised or low-to-medium-risk flows>` | `<>` |
| R7 | Function creep (reuse for identification, surveillance, profiling) | `<>` | Severe | 1:1 API only; no embeddings returned; no gallery | `<purpose limitation in policy and contracts; access controls>` | `<>` |
| R8 | Biometric data in logs or analytics | `<>` | Significant | Service logs and metrics contain none | `<don't log images, base64 output, geometry or scores; review APM / error tracking tools>` | `<>` |
| R9 | API key theft enabling misuse of the service | `<>` | Significant | Keys hashed at rest, profile restrictions, per-project logging | `<secret store; never in the browser; rotation>` | `<>` |
| R10 | Lack of transparency or valid consent | `<>` | Significant | — | `<notice, consent record, alternative>` | `<>` |
| R11 | Excessive retention | `<>` | Significant | Service retains nothing | `<schedule, automated deletion>` | `<>` |
| R12 | Model licensing or legal-review issues affecting lawfulness | `<>` | `<>` | Licence and training-data statements in the manifest; review status reported in every response (currently `pending`) | `<confirm review status before go-live>` | `<>` |

## 7. Automated decision-making and human oversight

| Question | Answer |
|---|---|
| Is any decision with legal or similarly significant effect taken solely by the outcome? | `<yes/no>` |
| What happens on `verified`? | `<>` |
| What happens on `rejected`? | `<retry limit, then human review / alternative>` |
| What happens on `review`? | `<who reviews, within what time, with what training and tools>` |
| What happens on `retake`? | `<retry limit, guidance shown>` |
| How can a person contest a decision? | `<>` |
| How are reviewers prevented from over-trusting the system? | `<training; the score is not shown as a percentage; provisional calibration explained>` |

*While `calibration.status` is `provisional`, the `targetFmr` of an operating point is a target, not
a measured rate. Keep a human in the loop for consequential decisions until the profile is
`calibrated`.*

## 8. Security measures

| Area | Measure |
|---|---|
| Transport | `<TLS browser-backend; loopback or TLS backend-service>` |
| Authentication | `<user auth in your app; API key per project for the service>` |
| Key management | `<secret store, rotation period, revocation procedure>` |
| Storage (if any) | `<encryption at rest, key management, access control, logging of access>` |
| Service host hardening | `<least-privilege service account, file permissions on models and profiles, firewall, /metrics restricted>` |
| Monitoring | `<health, error rates, outcome mix>` |
| Incident response | `<breach notification process and deadlines: GDPR 72 h to the authority; PDPL notification to the UAE Data Office as required>` |
| Testing | `<pen test, load test>` |

## 9. Consultation

| Stakeholder | Date | Outcome |
|---|---|---|
| DPO | `<>` | `<>` |
| Information security | `<>` | `<>` |
| Legal | `<>` | `<>` |
| Data subjects or their representatives (where appropriate) | `<>` | `<>` |
| Regulator (prior consultation if high residual risk remains: GDPR Art. 36; PDPL as applicable) | `<>` | `<>` |

## 10. Conclusion and sign-off

| Item | Value |
|---|---|
| Residual risk level | `<low / medium / high>` |
| Processing may proceed? | `<yes / yes with conditions / no>` |
| Conditions and actions | `<owner, due date>` |
| Approved by | `<name, role, date>` |
| DPO opinion | `<>` |

## 11. Re-assessment triggers

Review this DPIA when any of these change:

- A new Veriface profile version, model pack, or catalogue version (outcomes can change;
  see [versioning and upgrades](versioning-and-upgrades.md)).
- `calibration.status` moves from `provisional` to `calibrated`, or the operating point changes.
- Model legal review status changes.
- New purposes, new data subject groups, new countries, or a new capture setting (for example
  moving from supervised to remote).
- You start storing images, responses or any data you did not store before.
- A security incident or a pattern of complaints.
