# Filesystem protection policy

Protects resources that conventionally hold credentials.

These patterns are a convention of one ecosystem, not a fact about change
control, which is why they live here rather than in Draft. A project storing its
secrets elsewhere is not protected by this package, and should say so in its own
policy.

Only Draft's own `.draft/**` is protected intrinsically, by Core, and no
contribution can widen or narrow that.
