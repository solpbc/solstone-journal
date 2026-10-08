# usage: . scripts/env.sh <goversion>
R=${GOLIC_ROOT:-/var/tmp/golic-r1}
export GOROOT=$R/tc/go$1/go
export PATH=$GOROOT/bin:/usr/bin:/bin
export GOPATH=$R/gopath
export GOMODCACHE=$R/modcache
export GOCACHE=$R/gocache
export GOTOOLCHAIN=local
export GOFLAGS=
export GOPROXY=https://proxy.golang.org,direct
export GOSUMDB=sum.golang.org
export GONOSUMDB= GOPRIVATE= GONOPROXY= GOINSECURE=
export CGO_ENABLED=0
