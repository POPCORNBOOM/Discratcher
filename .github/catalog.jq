# Builds the app's library from the raw issues the workflow fetched:
# an index small enough to sync often, and the geometry only a download needs.

# The JSON block the issue template fills in.
def payload:
  (.body // "")
  | (capture("```json\\s*(?<data>[\\s\\S]*?)```")?) as $fenced
  | ($fenced.data | fromjson?);

def printable: type == "string" and length > 0;
def finite: [.totalSectors, .channelBitLengthNm, .innerRadiusMm, .outerRadiusMm, .pitchLinear, .pitchQuadratic, .pitchCubic, .basePitchUm, .rmsResidualDeg];

# Anyone can open an issue: keep only the ones carrying the full numeric shape.
def preset($issue):
  $issue | payload as $p
  | select($issue.pull_request == null and ($p | type) == "object")
  | select(($p | finite | map(type == "number") | all) and ($p.name | printable) and ($p.stamperCode | printable))
  | select(($p.measuredSpanMm | type) == "array" and ($p.measuredSpanMm | length) == 2 and ($p.measuredSpanMm | map(type == "number") | all))
  | $p + {
      id: "issue-\($issue.number)",
      mediaId: ($p.mediaId // "" | tostring),
      brand: ($p.brand // "" | tostring | .[0:40]),
      driveModel: ($p.driveModel // "" | tostring),
      createdAt: ($p.createdAt // "" | tostring)
    };

def vote($issue):
  $issue | payload as $p
  | select($issue.pull_request == null and ($p | type) == "object" and $issue.user.login != null)
  | ($p.target | tostring) as $target
  | ($p.score) as $score
  | select($target != "" and ($score | type) == "number")
  | select(($score | floor) == $score and $score >= 1 and $score <= 5)
  | { target: $target, score: $score, account: $issue.user.login, number: $issue.number };

# One vote per account per calibration: the newest issue wins, which is the highest number.
def ratings:
  [ .[] | vote(.) ]
  | group_by([.account, .target])
  | map(max_by(.number))
  | group_by([.target])
  | map({ key: .[0].target, value: { average: ((map(.score) | add) / length), count: length } })
  | from_entries;

[ ($c[0] // [])[] as $issue | { issue: $issue, preset: (preset($issue)) } ]
| map(select(.preset != null))
| . as $items
| ($r[0] // [] | ratings) as $ratings
| {
    index: [
      $items[] | .issue as $issue | .preset as $p | {
        id: $p.id,
        name: $p.name,
        stamperCode: $p.stamperCode,
        mediaId: $p.mediaId,
        brand: $p.brand,
        basePitchUm: $p.basePitchUm,
        size: (if $p.outerRadiusMm > 45 then 12 else 8 end),
        createdAt: $p.createdAt,
        contributor: ($issue.user.login // ""),
        verified: ([$issue.labels[].name] | index("verified") != null),
        rating: ($ratings[$issue.number | tostring] // null)
      }
    ],
    values: [ $items[].preset ]
  }
