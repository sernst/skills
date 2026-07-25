function Test-StrictSemVer {
    param([Parameter(Mandatory = $true)] [string] $Version)

    $numeric = '(?:0|[1-9]\d*)'
    $alphaNumeric = '(?:[0-9A-Za-z-]*[A-Za-z-][0-9A-Za-z-]*)'
    $identifier = "(?:$numeric|$alphaNumeric)"
    return $Version -match "^$numeric\.$numeric\.$numeric(?:-$identifier(?:\.$identifier)*)?$"
}
