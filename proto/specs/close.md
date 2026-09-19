# Close Protocol

`draft maintenance remove-project` removes Draft metadata and leaves user project files unchanged. It refuses pending unsafe state by default. `draft maintenance remove-project --force` may remove Draft metadata despite pending ChangePacks, but still must not delete project files.
