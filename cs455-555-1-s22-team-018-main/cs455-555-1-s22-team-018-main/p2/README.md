# Project 2: Identity Server (Phase 1)

* Author: Colin Reeder, Yoonsu Ra
* Class: CS455/555 [Distributed Systems] Section #001 Spring 2022

## Overview

Create a server that uses RMI (or RPC) to keep track of user id, login name, real name, and password on a database. 

## Guide through the code

### server/src/main.rs
1. Start up server
2. Handle queries from client
3. Send results of queries to client (only when results are necessary such as for look ups and gets)

### client/src/main.rs
1. Pass queries with commands and arguments to server
2. Receive and display results from queries

### common/src/lib.rs
1. List of RPC functions


## Building and running the code

### Prerequisites
- Rust installed  
- Visual Studio

### Running Server-
1. Change to project directory.
2. Change to server directory.
3. Run command "cargo run".

### Running Client- (on seperate console)
1. Change to project directory.
2. Change to server directory.
3. Run command "cargo run -- --\<command\> \<arugments\>".

### Rustdoc
- cargo doc (file ends up in target/doc)

## Testing
- Different situations tested in CLI (expected results from tests, expected client output for good inputs, expected errors from bad inputs)  
	* wrong password
	* putting in password for user without password
	* leaving out password for user with password
	* create account with existing login name
	* modify non-existing login name
	* modify login name to itself
	* reverse-/lookup on non existing uuid/login name
	


## Reflection
Yoonsu: Not as much struggles as p1 using Rust, but still needed a lot of assistance to understand syntax, especially in relation to database related coding.

Colin: We used a library to provide the RPC abstraction, but unfortunately it didn't support TLS servers natively, so I had to fork it.
I didn't contribute the changes back because I don't think it's the right way to go about the implementation, but it was quick to do and should work.
I wasn't able to figure out how to pin a certificate so I did have to make the client accept all certificates (but I imagine a real deployment would have domain names and use CAs)

## Roles
- Commands:
	* create *Yoonsu
	* lookup *Colin
	* reverse-lookup *Yoonsu
	* modify *Yoonsu
	* delete *Yoonsu
	* get *Colin
- Security:
	* Hashed passwords *Colin
	* TLS *Colin
- Tests:
	* Unit Tests *Colin:
		- Database
		- Rpc implementation
- Misc:
	* Allow specifying server host and port as CLI args *Colin
	* Add rustdoc comments *Yoonsu
	* Write Makefile *Yoonsu
	* Create video (https://drive.google.com/file/d/13ADUWOelSrGK-6Oe-G4b-F-20ridAggw/view?usp=sharing)
	* Write README.md *Yoonsu
