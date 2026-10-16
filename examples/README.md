# Synthetic examples

[observation.json](observation.json) illustrates the [draft observation contract](../schemas/observation.schema.json). It contains invented IDs, an all-zero placeholder hash, and no attached image. It is not a capture artifact, provider response, credential, or runtime compatibility test.

The schema checks structure. Runtime code must independently enforce scope containment, lease validity, epoch, timestamps, byte limits, session binding, and consistency between dimensions and transforms. The current documentation checker checks a few example consistency rules; it is not a full JSON Schema validator.
