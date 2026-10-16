# Music (example design doc)

The ensemble an island-travel theme is arranged for: `compose_arrange`
with `musicMd` reads this table, adds the instruments and layers it names,
and gives each its role. `calm` plays at every intensity; `lively` joins
from intensity 3 and adds the motion: eighth-note plucks, hand drums, a
second voice on the tune and a horn counter-line.

## Ensemble

| Id | Patch | Role | Layer | Velocity | Subdivision | Keys | Density |
|---|---|---|---|---|---|---|---|
| clarinet | clarinet | melody | calm | 0.75 | | | |
| harp | harp | arp | calm | 0.45 | 1 | | |
| cello | cellos_pizz | bass | calm | 0.72 | | | |
| violas | violas | pad | calm | 0.34 | | | |
| flute | flute | melody | lively | 0.66 | | | |
| glock | glockenspiel | sparkle | lively | 0.5 | | | |
| horn | horn | counter | lively | 0.5 | | | |
| pizz | violins_pizz | pulse | lively | 0.55 | 0.5 | | |
| bongos | bongos | percussion | lively | 0.6 | | 63/60 | normal |
| shaker | shaker | percussion | lively | 0.4 | | 64/65 | busy |
