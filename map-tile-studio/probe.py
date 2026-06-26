from osgeo import gdal, osr
gdal.UseExceptions()
ds=gdal.Open(r'C:/Users/Rezwan/AppData/Local/MapTileStudio/maps/COGTest-cog.tif'); gt=ds.GetGeoTransform()
cx=gt[0]+gt[1]*ds.RasterXSize/2; cy=gt[3]+gt[5]*ds.RasterYSize/2
t=osr.CoordinateTransformation(ds.GetSpatialRef(), osr.SpatialReference(); ) if False else None
H=20037508.342789244
import math
for z in (15,16,17,18):
    n=2**z; tm=2*H/n
    x=int((cx+H)/tm); y=int((H-cy)/tm)
    print(f'{z}/{x}/{y}')
