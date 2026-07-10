# Close Protocol

`draft close` removes Draft metadata and leaves user project files unchanged. It
refuses pending unsafe state by default. `draft close --force` may remove Draft
metadata despite pending packs, but still must not delete project files.
