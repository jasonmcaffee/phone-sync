Let's create a new project for syncing my iphone to a service we build.

we want a native ios swift app that is always running (in background, screen off, etc) in order to synce
our photos and videos from the camera to the server.  Create the app in this dir in a subdir called 
phone-sync-app.  The ui should be similar to the photos app, and show a grid of images and videos, that i can
click to expand and play, and there should be an indicator to show if the item has synced.
there should be a manual sync button as well, to manually save any media to the server that hasn't been synced.

The backend service should be written in rust, and provide all the endpoints and features needed to accept ios
images and videos, and store them to the local file system.

We will develop this on a macbook, but the service will ultimately run on a windows 11 machine.

We will want authentication. use user "jason" password "modestMouse1!" initially for backend service access.
Have the ui show a sign in page.  Token should last a year.

Initially we will point this to local service url via ip address and port, but ultimately want this 
pointing to phone.jasonmcaffee.com.

create the tdd, then fully implement.  verify through simulator app that things work as expected.

The goal is to have a phone app/service that constantly backs up my photos and videos to the server as I take pictures/video, 
without me having to manually open the app.
