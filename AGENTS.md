> **Reprise de la campagne de vérification : lis d'abord `docs/REPRISE.md`.** Branche réelle : `local/wian` (jamais `main`) ; la CI ne se lance que sur les Pull Requests, donc on travaille par petites PR vers `local/wian`, et la ligne « Commit directly to main; no PRs » ci-dessous ne s'applique pas à cette campagne.

- Follow [architecture and standards](Docs/Architecture.md).
- Keep designs simple; centralize duplicated behavior.
- Document functions and ownership contracts.
- Isolate platform services; preserve product behavior.
- Verify through GitHub Actions; do not run tests locally.
- Commit directly to main; no PRs.
