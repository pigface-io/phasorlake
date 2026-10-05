-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#BIDDAYOFFER_D#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS BIDDAYOFFER_D owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#BIDDAYOFFER_D#FILE01#202608010000.CSV
APPEND INTO TABLE BIDDAYOFFER_D
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
SETTLEMENTDATE DATE "YYYY/MM/DD HH24:MI:SS",
DUID CHAR(10),
BIDTYPE CHAR(10),
DIRECTION CHAR(20),
BIDSETTLEMENTDATE DATE "YYYY/MM/DD HH24:MI:SS",
OFFERDATE DATE "YYYY/MM/DD HH24:MI:SS",
VERSIONNO FLOAT EXTERNAL,
PARTICIPANTID CHAR(10),
DAILYENERGYCONSTRAINT FLOAT EXTERNAL,
REBIDEXPLANATION CHAR(500),
PRICEBAND1 FLOAT EXTERNAL,
PRICEBAND2 FLOAT EXTERNAL,
PRICEBAND3 FLOAT EXTERNAL,
PRICEBAND4 FLOAT EXTERNAL,
PRICEBAND5 FLOAT EXTERNAL,
PRICEBAND6 FLOAT EXTERNAL,
PRICEBAND7 FLOAT EXTERNAL,
PRICEBAND8 FLOAT EXTERNAL,
PRICEBAND9 FLOAT EXTERNAL,
PRICEBAND10 FLOAT EXTERNAL,
MINIMUMLOAD FLOAT EXTERNAL,
T1 FLOAT EXTERNAL,
T2 FLOAT EXTERNAL,
T3 FLOAT EXTERNAL,
T4 FLOAT EXTERNAL,
NORMALSTATUS CHAR(3),
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS",
MR_FACTOR FLOAT EXTERNAL,
ENTRYTYPE CHAR(20))
-- End of Script
