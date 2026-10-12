# 4. Licensing

## Status

Proposed. The owner decides; nothing here is accepted. This is not legal advice.

## The test

Tehuti's mission is accessibility: making it available to more people. Every licensing choice must serve that. The owner keeps full control over licensing. In return, every licensing decision, past and expected, is recorded in public ADRs. Anyone can check each one against the mission.

## Decisions

### Core

MPL-2.0. The licence works file by file. Changes to Tehuti's files must be shared. New files in a larger work may use any licence [1].

Rejected:

- GPL: forces whole programs open, which keeps vendors away.
- MIT alone: lets improvements to the core stay closed.

### Protocol, schemas and SDKs

Apache-2.0, or MIT OR Apache-2.0. Vendors may copy the SDK into closed plugins. Apache-2.0 adds a patent grant that ends for anyone who sues over patents in the work [2].

Rejected: the core's licence. It would put file-level duties on vendor code.

### Docs

Each doc takes the licence of the part it describes.

### Third-party plugins and the directory (#29)

Plugins run as separate programs (#1) and may use any licence. A future official directory lists only plugins that meet the trademark's privacy and security conditions. Unlisted plugins are their authors' responsibility. Tehuti asks the user before an unlisted plugin runs for the first time.

Rejected: licence terms that limit how plugins are used. The Open Source Definition forbids limits on fields of use [3].

### Contributions

Contributors donate their copyright to the owner. The pull request template says so. The owner promises that licensing choices will benefit the project, judged by the test above. Those who do not want to donate can write a plugin instead.

Some countries do not allow copyright to be given away. Germany is one [4]. There the fallback is an exclusive, perpetual, irrevocable licence that includes the right to relicense.

Rejected: a binding licence floor. The owner keeps full freedom; public records give the assurance.

### Trademark, security and privacy

These are set by trademark policy and `SECURITY.md`, not by the licence. Only builds that meet the published conditions may be called Tehuti. The owner registers the trademark with counsel before inviting vendors.

Rejected: privacy or security terms in the licence. They would be use limits [3].

## Later versions of MPL

MPL-2.0 lets anyone use the code under a later MPL version from Mozilla [1]. Counsel should confirm this is acceptable.

## Left for counsel

- Wording of the donation and of the fallback licence. German law keeps some rights with the author even under an exclusive licence [4].
- Trademark registration and the conditions for using the name.
- Whether to choose Apache-2.0 alone or MIT OR Apache-2.0 for the SDKs.
- The MPL later-versions clause.

## Sources

1. Mozilla Public License 2.0, sections 1.10, 3.3 and 10.2: https://www.mozilla.org/en-US/MPL/2.0/
2. Apache License 2.0, section 3: https://www.apache.org/licenses/LICENSE-2.0
3. Open Source Definition, criterion 6: https://opensource.org/osd
4. German Copyright Act, sections 29 and 31: https://www.gesetze-im-internet.de/englisch_urhg/englisch_urhg.html
