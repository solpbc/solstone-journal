# Windows CompareStringOrdinal ASCII corpus (2026-08-23)

This is a pinned capture from Windows. Native Windows tests exercise the
journal substrate, but this corpus itself does not claim arbitrary
Unicode-to-Unicode equivalence — it proves only the 65 listed pairs, and the
negative test vectors in `name_admission.rs` confirm nothing else folds.

It is crate-local evidence for `solstone-core-journal-io`, not a
`core/fixtures/` oracle. Those fixtures are a different genre:
deleted-Python-generator pins (see `core/fixtures/FROZEN.md`). Do not
regenerate this file.

- Platform: Windows 11 build 26200
- API: `CompareStringOrdinal` with `bIgnoreCase = TRUE`
- Comparison: each of 39 admitted ASCII units against all 1,112,064 Unicode scalar values
- Denominator: 1,112,064
- Result: exactly 65 expected mappings, zero unexpected mappings
- Producer SHA-256: `4b8199b265e41c4ecd66f97d70cae65ed7ca3ca5dc1545a13c2877073ab513f5`
- Captured-output SHA-256: `ba9ff605b9760ac62be0359c153f839264ff9b35f901a1dc02d5f5cfadd6b9ae`

## Mapping

```
a:U+0041,a:U+0061,b:U+0042,b:U+0062,c:U+0043,c:U+0063,d:U+0044,d:U+0064,e:U+0045,e:U+0065,f:U+0046,f:U+0066,g:U+0047,g:U+0067,h:U+0048,h:U+0068,i:U+0049,i:U+0069,j:U+004A,j:U+006A,k:U+004B,k:U+006B,l:U+004C,l:U+006C,m:U+004D,m:U+006D,n:U+004E,n:U+006E,o:U+004F,o:U+006F,p:U+0050,p:U+0070,q:U+0051,q:U+0071,r:U+0052,r:U+0072,s:U+0053,s:U+0073,t:U+0054,t:U+0074,u:U+0055,u:U+0075,v:U+0056,v:U+0076,w:U+0057,w:U+0077,x:U+0058,x:U+0078,y:U+0059,y:U+0079,z:U+005A,z:U+007A,0:U+0030,1:U+0031,2:U+0032,3:U+0033,4:U+0034,5:U+0035,6:U+0036,7:U+0037,8:U+0038,9:U+0039,.:U+002E,_:U+005F,-:U+002D
```
