# testdata/

Only **licence-clean** images belong here:
- **Public-domain portraits**, for example US federal government works such as NASA astronaut portraits.
- **Synthetic documents** that the lab generates by compositing those portraits onto clearly fake "SPECIMEN" card layouts.

Every file needs a provenance entry in `LICENSES.md`: source URL, licence, and SHA-256. Keep the set small (tens of images, downscaled).

`golden/` holds reference outputs that the lab exports from the official MediaPipe and ONNX runs. Each one carries its own tolerances. The Rust golden tests read them.

Never add real selfies or real identity documents.
