module github.com/howlerops/slate-orm/examples/deployed/go

go 1.24

require github.com/howlerops/slate-orm/clients/go v0.0.0

require (
	golang.org/x/net v0.34.0 // indirect
	golang.org/x/sys v0.29.0 // indirect
	golang.org/x/text v0.21.0 // indirect
	google.golang.org/genproto/googleapis/rpc v0.0.0-20250115164207-1a7da9e5054f // indirect
	google.golang.org/grpc v1.71.0 // indirect
	google.golang.org/protobuf v1.36.5 // indirect
)

replace github.com/howlerops/slate-orm/clients/go => ../../../clients/go
