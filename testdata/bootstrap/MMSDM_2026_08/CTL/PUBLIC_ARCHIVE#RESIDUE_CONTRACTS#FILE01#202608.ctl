-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#RESIDUE_CONTRACTS#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS RESIDUE_CONTRACTS owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#RESIDUE_CONTRACTS#FILE01#202608010000.CSV
APPEND INTO TABLE RESIDUE_CONTRACTS
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
CONTRACTYEAR FLOAT EXTERNAL,
QUARTER FLOAT EXTERNAL,
TRANCHE FLOAT EXTERNAL,
CONTRACTID CHAR(30),
STARTDATE DATE "YYYY/MM/DD HH24:MI:SS",
ENDDATE DATE "YYYY/MM/DD HH24:MI:SS",
NOTIFYDATE DATE "YYYY/MM/DD HH24:MI:SS",
AUCTIONDATE DATE "YYYY/MM/DD HH24:MI:SS",
CALCMETHOD CHAR(20),
AUTHORISEDDATE DATE "YYYY/MM/DD HH24:MI:SS",
AUTHORISEDBY CHAR(15),
NOTIFYPOSTDATE DATE "YYYY/MM/DD HH24:MI:SS",
NOTIFYBY CHAR(15),
POSTDATE DATE "YYYY/MM/DD HH24:MI:SS",
POSTEDBY CHAR(15),
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS",
DESCRIPTION CHAR(80),
AUCTIONID CHAR(30))
-- End of Script
