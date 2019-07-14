Create really small PNG files, containing very large bitmaps. This uses 1 bit/pixel grayscale encoding, eg. each bit is a pixel that's either fully black (0) or fully white (1), and as it turns out, streams of zeroes compress very, very well.

Open one to make your viewer run out of RAM. Throw them at your servers to test that they handle very large images properly.
