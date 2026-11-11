# Builds the app's library from the raw issues the workflow fetched: one small
# index for searching, and one folder per calibration holding only that
# calibration's geometry, votes and pictures.

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

# One vote per account per calibration: the newest issue wins, which is the highest
# number, so re-rating overwrites the key instead of counting twice.
def votes:
  [ .[] | vote(.) ]
  | group_by([.account, .target])
  | map(max_by(.number))
  | reduce .[] as $v ({}; .[$v.target][$v.account] = $v.score);

def summary($votes):
  ($votes | length) as $count
  | if $count == 0 then null else { average: ([$votes[]] | add / $count), count: $count } end;

# Pictures the contributor dropped into the issue; the URLs are stable, so the folder keeps those.
# GitHub writes them as markdown when typed and as <img> tags when dropped in.
def images($issue):
  ($issue.body // "") as $body
  | [
      ($body | scan("!\\[[^\\]]*\\]\\(([^)]+)\\)") | .[0]),
      ($body | scan("<img[^>]+src=\"([^\"]+)\"") | .[0]),
      ($body | scan("<img[^>]+src='([^']+)'") | .[0])
    ] | unique;

[ ($c[0] // [])[] as $issue | { issue: $issue, preset: (preset($issue)) } ]
| map(select(.preset != null))
| . as $items
| ($r[0] // [] | votes) as $votes
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
        rating: summary($votes[$issue.number | tostring] // {})
      }
    ],
    files: (
      reduce $items[] as $item ({};
        ($item.issue.number | tostring) as $n
        | .["library/issue-\($n)/data.json"] = $item.preset
        | .["library/issue-\($n)/ratings.json"] = ($votes[$n] // {})
        | (images($item.issue)) as $pictures
        | if ($pictures | length) > 0 then .["library/issue-\($n)/media.json"] = $pictures else . end
      )
    )
  }
