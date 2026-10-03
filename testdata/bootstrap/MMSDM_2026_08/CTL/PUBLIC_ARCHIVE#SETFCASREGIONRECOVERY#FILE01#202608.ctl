-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#SETFCASREGIONRECOVERY#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS SETFCASREGIONRECOVERY owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#SETFCASREGIONRECOVERY#FILE01#202608010000.CSV
APPEND INTO TABLE SETFCASREGIONRECOVERY
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
SETTLEMENTDATE DATE "YYYY/MM/DD HH24:MI:SS",
VERSIONNO FLOAT EXTERNAL,
BIDTYPE CHAR(10),
REGIONID CHAR(10),
PERIODID FLOAT EXTERNAL,
GENERATORREGIONENERGY FLOAT EXTERNAL,
CUSTOMERREGIONENERGY FLOAT EXTERNAL,
REGIONRECOVERY FLOAT EXTERNAL,
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS",
REGION_ACE_MWH FLOAT EXTERNAL,
REGION_ASOE_MWH FLOAT EXTERNAL,
REGIONRECOVERYAMOUNT_ACE FLOAT EXTERNAL,
REGIONRECOVERYAMOUNT_ASOE FLOAT EXTERNAL,
REGIONRECOVERYAMOUNT FLOAT EXTERNAL)
-- End of Script
