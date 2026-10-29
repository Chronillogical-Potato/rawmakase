// Offline reference for RAWmakase's Lensfun tests: prints what Lensfun itself does with
// a calibration, for comparison, never linked into RAWmakase. Reads a database (one
// synthetic lens) on stdin; arguments: focal crop width height aperture distance mode
// then points "x,y". Mode "v" prints the vignetting gain at each point; any of "d"
// and "t" prints the red, green and blue source coordinates of each output pixel with
// distortion and/or lateral CA corrected. Build against a Lensfun checkout, see
// make_reference.py.
#include "lensfun.h"
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <sstream>
#include <string>
int main(int argc, char **argv) {
  std::stringstream ss;
  ss << std::cin.rdbuf();
  std::string xml = ss.str();
  lfDatabase db;
  if (db.Load(xml.c_str(), xml.size()) != LF_NO_ERROR) return 2;
  const lfLens *lens = db.GetLenses()[0];
  float focal = atof(argv[1]), crop = atof(argv[2]);
  int w = atoi(argv[3]), h = atoi(argv[4]);
  float aperture = atof(argv[5]), distance = atof(argv[6]);
  const char *mode = argv[7];
  for (int i = 8; i < argc; i++) {
    float x, y;
    sscanf(argv[i], "%f,%f", &x, &y);
    lfModifier m(lens, focal, crop, w, h, LF_PF_F32);
    if (strchr(mode, 'v')) {
      int on = m.EnableVignettingCorrection(aperture, distance);
      float p[3] = {1, 1, 1};
      m.ApplyColorModification(p, x, y, 1, 1, LF_CR_3(RED, GREEN, BLUE), sizeof(p));
      printf("%d %.9g\n", on, p[1]);
    } else {
      int on = 0;
      if (strchr(mode, 'd')) on |= m.EnableDistortionCorrection();
      if (strchr(mode, 't')) on |= m.EnableTCACorrection();
      float out[6];
      m.ApplySubpixelGeometryDistortion(x, y, 1, 1, out);
      printf("%d %.9g %.9g %.9g %.9g %.9g %.9g\n", on, out[0], out[1], out[2], out[3], out[4], out[5]);
    }
  }
}
