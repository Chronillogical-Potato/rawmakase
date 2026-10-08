"""16-bit sRGB comparison metrics. Reduction occurs before Lab conversion.

Inputs must already have the same framing. This is a regression measurement,
not a claim of pixel-identical demosaicing.
"""
import numpy as np
from PIL import Image
import tiff16

def load(p):
 a=tiff16.read(p);h,w=a.shape[:2];size=(round(w*256/max(w,h)),round(h*256/max(w,h)))
 return np.stack([np.array(Image.fromarray(a[...,i].astype('float32')).resize(size,Image.Resampling.BOX)) for i in range(3)],-1).astype('float64')
def lab(a):
 lin=np.where(a<=.04045,a/12.92,((a+.055)/1.055)**2.4)
 xyz=lin@np.array([[.4124564,.3575761,.1804375],[.2126729,.7151522,.072175],[.0193339,.119192,.9503041]]).T/np.array([.95047,1,1.08883])
 f=np.where(xyz>216/24389,np.cbrt(xyz),(24389/27*xyz+16)/116)
 return np.stack([116*f[...,1]-16,500*(f[...,0]-f[...,1]),200*(f[...,1]-f[...,2])],-1)
def de00(x,y):
 l1,a1,b1=np.moveaxis(x,-1,0);l2,a2,b2=np.moveaxis(y,-1,0)
 cb=(np.hypot(a1,b1)+np.hypot(a2,b2))/2;g=.5*(1-np.sqrt(cb**7/(cb**7+25**7)))
 ap1=(1+g)*a1;ap2=(1+g)*a2;cp1=np.hypot(ap1,b1);cp2=np.hypot(ap2,b2)
 h1=np.degrees(np.arctan2(b1,ap1))%360;h2=np.degrees(np.arctan2(b2,ap2))%360
 dh=(h2-h1+180)%360-180;dh=np.where(cp1*cp2==0,0,dh)
 dH=2*np.sqrt(cp1*cp2)*np.sin(np.radians(dh/2));dl=l2-l1;dc=cp2-cp1
 lb=(l1+l2)/2;cb=(cp1+cp2)/2
 hb=np.where(abs(h1-h2)<=180,(h1+h2)/2,np.where(h1+h2<360,(h1+h2+360)/2,(h1+h2-360)/2));hb=np.where(cp1*cp2==0,h1+h2,hb)
 T=1-.17*np.cos(np.radians(hb-30))+.24*np.cos(np.radians(2*hb))+.32*np.cos(np.radians(3*hb+6))-.2*np.cos(np.radians(4*hb-63))
 theta=30*np.exp(-((hb-275)/25)**2);rc=2*np.sqrt(cb**7/(cb**7+25**7))
 sl=1+.015*(lb-50)**2/np.sqrt(20+(lb-50)**2);sc=1+.045*cb;sh=1+.015*cb*T;rt=-np.sin(np.radians(2*theta))*rc
 return np.sqrt((dl/sl)**2+(dc/sc)**2+(dH/sh)**2+rt*(dc/sc)*(dH/sh))
def shift(a,dy,dx):
 h,w=a.shape[:2];yy,xx=np.mgrid[:h,:w];yy=(yy+dy).clip(0,h-1);xx=(xx+dx).clip(0,w-1);y=yy.astype(int);x=xx.astype(int);fy=(yy-y)[...,None];fx=(xx-x)[...,None];y1=np.minimum(y+1,h-1);x1=np.minimum(x+1,w-1)
 return (a[y,x]*(1-fx)+a[y,x1]*fx)*(1-fy)+(a[y1,x]*(1-fx)+a[y1,x1]*fx)*fy
