# testdata provenance

Every file under `testdata/` is listed here. This file is rendered from `provenance.json` by `python -m facelab testdata ...`; do not edit it by hand. Golden outputs (`golden/`) are derived data computed from these images.

## Portraits (`images/`)

Source: Wikimedia Commons. At build time, each page was checked for the `{{PD-USGov-NASA}}` licence tag. That
tag means a photograph taken by a NASA employee as part of their official duties, which is in the public domain
in the United States (17 U.S.C. 105). Each original was verified against the SHA-1 Commons reported at curation
(2026-10-01), then downscaled to a long side of 1024 px (LANCZOS), re-encoded as JPEG quality 92 and stripped of
all metadata.

Public domain covers copyright only. NASA's media guidelines say that images of identifiable people (here,
astronauts) must not be used to imply endorsement. These files are internal test fixtures only. Never use them
in marketing, customer demos or published material.

| id | file | size | sha256 | licence | author | source page | original sha1 | note |
|---|---|---|---|---|---|---|---|---|
| nasa-glover | `images/nasa-glover.jpg` | 768x1024 | `37c020be21d0018da041799bbdc201c8e3ac7ac2633a705468760cc53ecfce5b` | Public domain (PD-USGov-NASA) | NASA Robert Markowitz | https://commons.wikimedia.org/wiki/File:Victor_J._Glover_official_portrait.jpg | `704fc3f12fde82adb3bcd29b66b43e725382ded7` |  |
| nasa-watkins | `images/nasa-watkins.jpg` | 819x1024 | `6f72a3bc961388a2d5c4255adc5930bc02d9e2aa4032b45df74fec8b0146f47a` | Public domain (PD-USGov-NASA) | NASA/Bill Stafford Bill Stafford | https://commons.wikimedia.org/wiki/File:Jessica_Watkins_official_portrait.jpg | `459463a92a15f4a243c4edb20aafb7ff69af6574` |  |
| nasa-chari | `images/nasa-chari.jpg` | 768x1024 | `a6d50ad3a5328e0a8cbfe4c4a2ed648603de9b1d2bba5069f1d56ad7949b1a6f` | Public domain (PD-USGov-NASA) | Photographer: NASA / Robert Markowitz Robert Markowitz | https://commons.wikimedia.org/wiki/File:Raja_Chari_portrait.jpg | `20dbe4257f574a307aaaa0c58f9a4d1e567e84c7` |  |
| nasa-menon | `images/nasa-menon.jpg` | 819x1024 | `dfee2c3da2b664fb3cd4af3c7790e673c836afd3889154df1a6297f8bb1160df` | Public domain (PD-USGov-NASA) | NASA Robert Markowitz | https://commons.wikimedia.org/wiki/File:Anil_Menon_portrait.jpg | `08bd951d2b67620f4e8c20741b96f7bbf6c890aa` |  |
| nasa-moghbeli | `images/nasa-moghbeli.jpg` | 819x1024 | `ad21d9cf5e81d99c74736c8c6f049649f342b06c99a6f74c73ba1b218caa6812` | Public domain (PD-USGov-NASA) | NASA/Bill Stafford Bill Stafford | https://commons.wikimedia.org/wiki/File:Jasmin_Moghbeli_official_portrait.jpg | `69a05dd33b1724824c2a0f1e8484fcadd17e0b39` |  |
| nasa-almansoori | `images/nasa-almansoori.jpg` | 819x1024 | `6bcdab5ed4e72863ec1739baf7f0541b16438ba11e6a2f8e4ae551d7fcb46e31` | Public domain (PD-USGov-NASA) | Robert Markowitz | https://commons.wikimedia.org/wiki/File:Hazzaa_AlMansoori.jpg | `fff8b30845d588c5efff80aa9abe1926ce6835df` |  |
| nasa-rubio | `images/nasa-rubio.jpg` | 819x1024 | `0be01df5cd0fe795d78a535cf9af58eaf84e1387d14d571c33c7ec5c8b093621` | Public domain (PD-USGov-NASA) | NASA/Bill Stafford Bill Stafford | https://commons.wikimedia.org/wiki/File:Frank_Rubio_official_portrait.jpg | `89b5f2667652e0a378e8870634a0e7f62aa5a992` |  |
| nasa-ochoa | `images/nasa-ochoa.jpg` | 768x1024 | `3e702678889d2c349aa1770fb9a835326387b5181a53da21f58b5307701dc8f9` | Public domain (PD-USGov-NASA) | NASA/Bill Stafford Bill Stafford | https://commons.wikimedia.org/wiki/File:Ellen_Ochoa,_official_portrait.jpg | `8f3ce8d0769f93c14d514967ee5ee7f0db94911a` |  |
| nasa-hoshide | `images/nasa-hoshide.jpg` | 819x1024 | `78a9efe397b021e8e4cd0d789beb7d742527ab67fb3480e9ed49d162df424fad` | Public domain (PD-USGov-NASA) | NASA Robert Markowitz | https://commons.wikimedia.org/wiki/File:Akihiko_Hoshide,_official_portrait_(2020).jpg | `48bade0ffbb1e1e7c5e9bbce176cb6db5e0e39e9` |  |
| nasa-cardman | `images/nasa-cardman.jpg` | 768x1024 | `f628b71225f39c3101215ba6fd8e4ffae3e15ed6a7f64f0ee3833733df33e1ce` | Public domain (PD-USGov-NASA) | Photographer: NASA / Robert Markowitz Robert Markowitz | https://commons.wikimedia.org/wiki/File:Zena_Cardman_portrait.jpg | `db5ce17898a76a044e60dcecdbcac27ce46c84fe` |  |
| nasa-hammock | `images/nasa-hammock.jpg` | 768x1024 | `c58d5f30090b6fd04cd93f9d34b0d847be177b6fbaccdcb566a28b74e7014124` | Public domain (PD-USGov-NASA) | NASA/Robert Markowitz Robert Markowitz | https://commons.wikimedia.org/wiki/File:Christina_M._Hammock_portrait.jpg | `8d70b051fa71fa4282a96af64cd3250e5979a073` |  |
| nasa-cassada | `images/nasa-cassada.jpg` | 768x1024 | `c6cb907ca627571c013aba66f4c0816d25d57ca13bedc11f2c1f64fddd8b3254` | Public domain (PD-USGov-NASA) | NASA Robert Markowitz | https://commons.wikimedia.org/wiki/File:Josh_A._Cassada_official_portrait.jpg | `de102f0dc66bfd20c7b4842e2997b7607031fb7d` |  |
| nasa-hoburg | `images/nasa-hoburg.jpg` | 768x1024 | `d933e753fd4ab4a514cb096c209ad78fb03946835cfa19d2b581ce5ad2523c63` | Public domain (PD-USGov-NASA) | Photographer: NASA / Robert Markowitz Robert Markowitz | https://commons.wikimedia.org/wiki/File:Warren_Hoburg_portrait.jpg | `99cb4ab0d1b1c634b7a5d4d892b43c1bcd3e03bb` |  |
| nasa-hansen | `images/nasa-hansen.jpg` | 819x1024 | `e862a82918c13b3e168f4002dfc23706ab0cd0e9efdb6bc09b8ad3ecf345db38` | Public domain (PD-USGov-NASA) | NASA Robert Markowitz | https://commons.wikimedia.org/wiki/File:Jeremy_Hansen_official_portrait.jpg | `f2973af23d2bcfd7bb872defb186ed6e7be363cd` |  |
| nasa-harris | `images/nasa-harris.jpg` | 819x1024 | `3bad38781bcbae2b267721eb91722d96a471f55188e11198d19b77292054f0a1` | Public domain (PD-USGov-NASA) | NASA | https://commons.wikimedia.org/wiki/File:Bernard_A._Harris_-_Official_Portrait.jpg | `a8f4fc2e8ca79d05ec47193e8a8e2784b3d295fe` |  |
| nasa-isaacman | `images/nasa-isaacman.jpg` | 819x1024 | `39aaa645f141b6527c0db25bce8f8be5461042e8babe92b80becfd2d51128a70` | Public domain (PD-USGov-NASA) | NASABill Ingalls | https://commons.wikimedia.org/wiki/File:Jared_isaacman_official_portrait.jpg | `ecce61b429447f5e9e19c87b630bb7c938b37852` |  |
| nasa-caldwell | `images/nasa-caldwell.jpg` | 819x1024 | `14d1f8eb023d65ad7ddb9a45bed97fbe7fd6488a4ccac1d3eb5fbd48643df00d` | Public domain (PD-USGov-NASA) | NASA | https://commons.wikimedia.org/wiki/File:Tracy_E_Caldwell_portrait.jpg | `88b1f7da42c021be891a7ace23c6ed45187d6976` | NASA JSC2007-E-033311 |
| nasa-lovell-1964 | `images/nasa-lovell-1964.jpg` | 819x1024 | `d70e6aa2da559e6afb148b9ddbb2914c07a3eac7f2b3720980afedf739760701` | Public domain (PD-USGov-NASA) | NASA | https://commons.wikimedia.org/wiki/File:Jim_Lovell_official_1964_portrait.jpg | `87e2c068825f04b04392ed73b7b5ec9bdb0664f0` | 1964 colour film scan |
| nasa-dominick | `images/nasa-dominick.jpg` | 768x1024 | `ab8c8af4a56be5cede55d27fc6f70c151c145f4d7fb776ea67962deca385ce71` | Public domain (PD-USGov-NASA) | Photographer: NASA / Robert Markowitz Robert Markowitz | https://commons.wikimedia.org/wiki/File:Matthew_Dominick_portrait.jpg | `9dba6e1fa1baa1572fb098c19e131d9b2b00e500` |  |
| nasa-newman | `images/nasa-newman.jpg` | 791x1024 | `025745fc4fc0e1f9a221ab4e240b1a2dd2aabe1dc1506627ca9c4f13dc1ef1e9` | Public domain (PD-USGov-NASA) | NASA/Bill Ingalls Bill Ingalls | https://commons.wikimedia.org/wiki/File:Dava_Newman,_official_portrait.jpg | `7f75dbd4ee20f412d5fb11f6e6250e132dbe8359` |  |
| nasa-meir | `images/nasa-meir.jpg` | 768x1024 | `f781b21dfba196ad2b0ac486d251a43fe762f5ab0209137ba19d32a029c540bc` | Public domain (PD-USGov-NASA) | NASA/Bill Ingalls Bill Ingalls | https://commons.wikimedia.org/wiki/File:Jessica_Meir_(portrait_crop).jpg | `a03b3b36ee26d5b048c328d38ad0e7530a67b11b` |  |
| nasa-exp49-crew | `images/nasa-exp49-crew.jpg` | 1024x819 | `4b06bc88412da504f56acd5a35c3dddf33e2f0f318dc14e3c01cdb45bc82e2f9` | Public domain (PD-USGov-NASA) | NASA Robert Markowitz | https://commons.wikimedia.org/wiki/File:Expedition_49_crew_portrait_with_astronaut_Kate_Rubins,_cosmonaut_Anatoly_Ivanishin,_and_astronaut_Takuya_Onishi.jpg | `4f0e865a65d97ce127e4cd6ef090582bb6eda6be` | group photo, three faces (multi-face case) |

## Derived pose fixtures (`images/`)

Geometric transforms of the portraits above, used to check pose sign conventions (testdata/golden/README.md). Licence: public domain, as the parent.

| id | file | size | sha256 | parent | transform |
|---|---|---|---|---|---|
| nasa-glover-flip | `images/nasa-glover-flip.jpg` | 768x1024 | `d080020e888886073b1322e92964b721924e32c8e639bab62f2503e7970f9d64` | nasa-glover | flip-horizontal |
| nasa-ochoa-flip | `images/nasa-ochoa-flip.jpg` | 768x1024 | `7e7716fc4303959c6d00a40d6822320073159351b81b2360e3f230ffad6d7d4c` | nasa-ochoa | flip-horizontal |
| nasa-glover-rollcw15 | `images/nasa-glover-rollcw15.jpg` | 768x1024 | `842332747a46f028747fe41de7686b4bb9330e73cfb860e0f4278ec87f39a6fa` | nasa-glover | rotate-cw-15 |
| nasa-glover-canvas-top | `images/nasa-glover-canvas-top.jpg` | 768x2048 | `d3ae773cadb72d51ec46be1bf4689f13029e86bf0ba15eccb313fb5e3a79d53a` | nasa-glover | canvas-top |
| nasa-glover-canvas-bottom | `images/nasa-glover-canvas-bottom.jpg` | 768x2048 | `d8000e423c8c63b55f00575a5b2d7977a0c72944783bf90efcb8baf26070a16f` | nasa-glover | canvas-bottom |
| nasa-almansoori-canvas-top | `images/nasa-almansoori-canvas-top.jpg` | 819x2048 | `949171961ac144ae2e530b532a23a75715304177f34440fe32c0c76a6d8fc5ad` | nasa-almansoori | canvas-top |
| nasa-almansoori-canvas-bottom | `images/nasa-almansoori-canvas-bottom.jpg` | 819x2048 | `099bae826c02bbc7597ecf8b2c16fd36fc7610d39ad977f6ae6ed0ab8552ae87` | nasa-almansoori | canvas-bottom |

## Synthetic documents (`documents/`)

Generated by `python -m facelab testdata documents` from the portraits above. Every card is a fictitious
"SPECIMEN" layout drawn by the lab. It carries no real issuer's design, emblem, security feature or number
format, and all names and numbers are invented. Licence: same as the source portrait (public domain); the
synthetic layout is the Veriface authors' own work.

| id | file | size | sha256 | source portrait(s) | variant | expected |
|---|---|---|---|---|---|---|
| doc-watkins | `documents/doc-watkins.jpg` | 1600x1200 | `b5c673cf8098fc1f8b6cb07e18abdd097620dc3968d6fff94e0aefe758691e14` | nasa-watkins | upright | main 1, ghost 1, rotation 0 |
| doc-almansoori | `documents/doc-almansoori.jpg` | 1600x1200 | `fd2d931438e1512908f934e5c21fa44626f7fbd86f25df22a33cae459c259aa9` | nasa-almansoori | upright | main 1, ghost 1, rotation 0 |
| doc-menon | `documents/doc-menon.jpg` | 1600x1200 | `9d0b4def19b7518f4fe959977e44f3aedd8300714e65864d069f6614f8bc964f` | nasa-menon | upright | main 1, ghost 1, rotation 0 |
| doc-ochoa-scan | `documents/doc-ochoa-scan.jpg` | 1011x638 | `c967e988744cac59daad60279820f096e4a7b9935640f88ac817c1ddd23492f3` | nasa-ochoa | flat scan, card only | main 1, ghost 1, rotation 0 |
| doc-watkins-rot90 | `documents/doc-watkins-rot90.jpg` | 1200x1600 | `40dc410a964a21bfd2d45b4766746552c1a0e487a7ccfc3f24a467c3866e5053` | nasa-watkins | rotated 90 cw | main 1, ghost 1, rotation 90 |
| doc-watkins-rot180 | `documents/doc-watkins-rot180.jpg` | 1600x1200 | `831f19a7fd6435d0035c6d9d3806d407f89501fdf1d35b7320a685b2e77e6203` | nasa-watkins | rotated 180 | main 1, ghost 1, rotation 180 |
| doc-watkins-rot270 | `documents/doc-watkins-rot270.jpg` | 1200x1600 | `74e7d6079919b52bd9e7bb8c4767e19215cbd3c4ee258b8700be555e6b58756f` | nasa-watkins | rotated 270 cw | main 1, ghost 1, rotation 270 |
| doc-almansoori-perspective | `documents/doc-almansoori-perspective.jpg` | 1600x1200 | `f9f8a30c9f823446dcd72c644666f7b7fe2ca7423d57346828da5bcdd2fce183` | nasa-almansoori | perspective warp | main 1, ghost 1, rotation 0 |
| doc-watkins-glare | `documents/doc-watkins-glare.jpg` | 1600x1200 | `7113ccc7553383dcf1c2ea43e42fe7414ed31d93dcec24b764ce6aba6d87af7e` | nasa-watkins | glare spot over portrait | main 1, ghost 1, rotation 0 |
| doc-menon-blur | `documents/doc-menon-blur.jpg` | 1600x1200 | `8b1dd916e909eaeebe04ee556c38b2fc6fad7fbb01d7f466c9e4013e5f242a84` | nasa-menon | gaussian blur sigma 4 | main 1, ghost 1, rotation 0 |
| doc-almansoori-lowres | `documents/doc-almansoori-lowres.jpg` | 400x300 | `d73d2437e03ef803e37c62a39a2368276ad9f2ea3601eccbc9fefbb78aa8d82b` | nasa-almansoori | low resolution (400x300) | main 1, ghost 1, rotation 0 |
| doc-watkins-jpeg | `documents/doc-watkins-jpeg.jpg` | 1600x1200 | `fe5bce26e77158efc97278f1bdf5d4d77767c57da814c2448e31210535182941` | nasa-watkins | heavy JPEG (quality 8) | main 1, ghost 1, rotation 0 |
| doc-noface | `documents/doc-noface.jpg` | 1600x1200 | `a3057e496d326a896bc8586f34ee2979de5d261492be51dc2a19ef584f4d6ddb` | - | no face (photo area blank) | main 0, ghost 0, rotation 0 |
| doc-twofaces | `documents/doc-twofaces.jpg` | 1600x1200 | `60e573203017a47d651b767ce8e73caf56f00f69d224c51cad00bf2a4e14891b` | nasa-chari, nasa-ochoa | two equal main portraits | main 2, ghost 0, rotation 0 |
