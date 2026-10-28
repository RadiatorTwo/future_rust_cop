Put an image of your Future Cop: L.A.P.D. CD (.iso or .bin) next to
fcop.exe (Linux: fcop), or put your copy of the game (PC version) into this folder:
the contents of the CD, an image of the CD (.iso or .bin), or your
installed game folder.

At its next start fcop finds the game, checks that it is the version
this port knows, and keeps only what the game needs:

  FCopLAPD\   FCopLAPD.exe, febmp.bin, missions (and your settings,
              key bindings and saved games)
  Video\      the videos (from the CD; the game runs without them)
  Textures\   replacement textures, if you have any (they are kept)
              ("fcop export" cuts the game's own into pictures to start from)

From a CD image these are taken out. Everything else in this folder
(the image too) is moved to the recycle bin (Linux: deleted).
